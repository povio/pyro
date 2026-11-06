//! `CSSStyleDeclaration` for inline styles (`element.style`) backed by the
//! `style` attribute, and `CSSStyleSheet` for `<style>`/`<link>`, whose rules
//! are parsed from the sheet text when a script first asks for them.
//!
//! Declaration blocks are Stylo's (`lp_dom::stylo::declarations`), as the
//! cascade parses them: invalid values are dropped, shorthands expand into
//! their longhands (`margin` sets `margin-top`, ...), values serialize as
//! CSSOM says (`#f00` reads back `rgb(255, 0, 0)`), and a write serializes
//! the whole block back. Properties Chrome has that the engine's Stylo does
//! not parse ([`UNPARSED`]) are kept as written. Computed styles come from
//! lp-dom's style engine via `getComputedStyle`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use lp_dom::cssom::{normalize_selector, parse_rules, Namespaces, ParsedRule};
use lp_dom::stylo::declarations::{self, DeclarationBlock};
use lp_dom::NodeId;
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::collections::NodeHold;
use crate::node::NodeData;
use crate::page::PageDom;
use crate::util::*;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declaration {
    pub name: String,
    pub value: String,
    pub important: bool,
}

/// Splits a declaration block respecting quotes, parentheses and comments.
/// The delimiters are ASCII, so the scan runs over bytes; a segment is
/// copied only for what it keeps.
pub fn parse_declarations(text: &str) -> Vec<Declaration> {
    let mut out: Vec<Declaration> = Vec::new();
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match quote {
            Some(q) => {
                if c == b'\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                b'"' | b'\'' => quote = Some(c),
                b'(' => depth += 1,
                b')' => depth -= 1,
                b';' if depth <= 0 => {
                    push_declaration(&mut out, &text[start..i]);
                    start = i + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    push_declaration(&mut out, &text[start..]);
    out
}

/// One `name: value [!important]` segment of [`parse_declarations`].
fn push_declaration(out: &mut Vec<Declaration>, seg: &str) {
    let seg = strip_comments(seg);
    let Some((name, value)) = seg.split_once(':') else { return };
    let name = name.trim();
    if name.is_empty() {
        return;
    }
    let name = if name.starts_with("--") { name.to_owned() } else { name.to_ascii_lowercase() };
    let mut value = value.trim();
    let mut important = false;
    if value.len() >= 10 && value.is_char_boundary(value.len() - 10) {
        let (rest, flag) = value.split_at(value.len() - 10);
        if flag.eq_ignore_ascii_case("!important") {
            value = rest.trim_end();
            important = true;
        }
    }
    if value.is_empty() {
        return;
    }
    out.retain(|d| d.name != name);
    out.push(Declaration { name, value: value.to_owned(), important });
}

fn strip_comments(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains("/*") {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return std::borrow::Cow::Owned(out),
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

pub fn serialize_declarations(decls: &[Declaration]) -> String {
    decls
        .iter()
        .map(|d| format!("{}: {}{};", d.name, d.value, if d.important { " !important" } else { "" }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// camelCase IDL attribute -> CSS property (`backgroundColor` ->
/// `background-color`, `webkitTransform` -> `-webkit-transform`, `cssFloat`
/// -> `float`).
pub fn camel_to_css(name: &str) -> String {
    if name == "cssFloat" {
        return "float".into();
    }
    let mut out = String::with_capacity(name.len() + 4);
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
        if i == 0 && name.starts_with("webkit") && c == 'w' {
            out.insert(0, '-');
        }
    }
    out
}

/// Properties Chrome supports that the engine's Stylo (servo build) may not
/// parse: their declarations are kept as written, so scripts that set and
/// read them back keep working. Only the names Stylo does not know count.
const UNPARSED: &[&str] = &[
    "-webkit-appearance",
    "-webkit-backdrop-filter",
    "-webkit-box-align",
    "-webkit-box-decoration-break",
    "-webkit-box-direction",
    "-webkit-box-flex",
    "-webkit-box-ordinal-group",
    "-webkit-box-orient",
    "-webkit-box-pack",
    "-webkit-font-smoothing",
    "-webkit-hyphens",
    "-webkit-line-clamp",
    "-webkit-mask",
    "-webkit-mask-image",
    "-webkit-mask-position",
    "-webkit-mask-repeat",
    "-webkit-mask-size",
    "-webkit-print-color-adjust",
    "-webkit-tap-highlight-color",
    "-webkit-text-fill-color",
    "-webkit-text-stroke",
    "-webkit-text-stroke-color",
    "-webkit-text-stroke-width",
    "-webkit-user-drag",
    "accent-color",
    "anchor-name",
    "appearance",
    "backdrop-filter",
    "box-decoration-break",
    "caret-color",
    "color-scheme",
    "contain-intrinsic-height",
    "contain-intrinsic-size",
    "contain-intrinsic-width",
    "content-visibility",
    "field-sizing",
    "forced-color-adjust",
    "hyphens",
    "interpolate-size",
    "line-clamp",
    "mask",
    "mask-clip",
    "mask-composite",
    "mask-image",
    "mask-mode",
    "mask-origin",
    "mask-position",
    "mask-repeat",
    "mask-size",
    "mask-type",
    "offset-distance",
    "offset-path",
    "offset-rotate",
    "overflow-anchor",
    "overflow-clip-margin",
    "overscroll-behavior",
    "overscroll-behavior-x",
    "overscroll-behavior-y",
    "paint-order",
    "position-anchor",
    "position-area",
    "position-try",
    "print-color-adjust",
    "resize",
    "ruby-position",
    "scroll-behavior",
    "scroll-margin",
    "scroll-margin-bottom",
    "scroll-margin-left",
    "scroll-margin-right",
    "scroll-margin-top",
    "scroll-padding",
    "scroll-padding-bottom",
    "scroll-padding-left",
    "scroll-padding-right",
    "scroll-padding-top",
    "scroll-snap-align",
    "scroll-snap-stop",
    "scroll-snap-type",
    "scrollbar-color",
    "scrollbar-gutter",
    "scrollbar-width",
    "shape-image-threshold",
    "shape-margin",
    "shape-outside",
    "tab-size",
    "text-anchor",
    "text-emphasis",
    "text-emphasis-color",
    "text-emphasis-position",
    "text-emphasis-style",
    "text-orientation",
    "text-underline-position",
    "text-wrap",
    "text-wrap-mode",
    "text-wrap-style",
    "view-transition-name",
    "white-space-collapse",
    "zoom",
];

/// A property Chrome has and Stylo does not parse (kept as written).
fn unparsed(name: &str) -> bool {
    UNPARSED.binary_search(&name).is_ok() && !lp_dom::stylo::is_property(name)
}

/// A CSS property name `CSSStyleDeclaration` reads and writes: one Stylo
/// knows, a custom property, or an [`UNPARSED`] one.
fn css_property(name: &str) -> bool {
    lp_dom::stylo::is_property(name) || unparsed(name)
}

/// The property a named member stands for: the camelCase IDL attribute
/// (`backgroundColor`, `webkitTransform`) or the dashed name itself
/// (`background-color`, as Chrome also exposes). Methods and attributes on
/// the prototype win because the named hook does not override builtins.
fn named_property(name: &str) -> Option<String> {
    if !name.chars().next().is_some_and(|c| c.is_ascii_lowercase() || c == '-') {
        return None;
    }
    let css = if name.contains('-') {
        if name.starts_with("--") || !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return None;
        }
        name.to_owned()
    } else {
        if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            return None;
        }
        camel_to_css(name)
    };
    css_property(&css).then_some(css)
}

/// A property name as `getPropertyValue`/`setProperty` take it: custom
/// properties case-sensitive, others ASCII-lowercased.
fn property_arg(name: String) -> String {
    if name.starts_with("--") {
        name
    } else {
        name.to_ascii_lowercase()
    }
}

pub struct StyleData {
    source: StyleSource,
}

/// A declaration block as CSSOM sees it: Stylo's declarations, then the
/// [`UNPARSED`] ones kept as written.
struct Block {
    parsed: DeclarationBlock,
    kept: Vec<Declaration>,
}

impl Block {
    fn parse(text: &str) -> Block {
        let kept = if text.is_empty() {
            Vec::new()
        } else {
            parse_declarations(text).into_iter().filter(|d| unparsed(&d.name)).collect()
        };
        Block { parsed: DeclarationBlock::parse(text), kept }
    }
    fn names(&self) -> Vec<String> {
        let mut names = self.parsed.names();
        names.extend(self.kept.iter().map(|d| d.name.clone()));
        names
    }
    fn css_text(&self) -> String {
        let parsed = self.parsed.css_text();
        let kept = serialize_declarations(&self.kept);
        match (parsed.is_empty(), kept.is_empty()) {
            (_, true) => parsed,
            (true, false) => kept,
            (false, false) => format!("{parsed} {kept}"),
        }
    }
}

/// What a CSSStyleDeclaration reads and writes.
enum StyleSource {
    /// An element's `style` attribute.
    Inline(NodeHold),
    /// The declaration block of a style rule of `owner`'s sheet.
    Rule { owner: NodeHold, node: Rc<RuleNode> },
}

impl StyleData {
    /// The block's text: the `style` attribute, or the rule's block.
    fn text(&self) -> String {
        match &self.source {
            StyleSource::Inline(el) => {
                el.page.dom.borrow().get_attribute(el.id, "style").ok().flatten().unwrap_or("").to_owned()
            }
            StyleSource::Rule { node, .. } => match &node.kind {
                RuleKind::Style { block, .. } => block.borrow().clone(),
                _ => String::new(),
            },
        }
    }
    fn block(&self) -> Block {
        Block::parse(&self.text())
    }
    /// Serializes `block` back into the attribute or the rule.
    fn write(&self, block: &Block) -> JsResult<()> {
        self.write_raw(block.css_text())
    }
    /// Writes `text` unless it is what the block already holds (`before`):
    /// an unchanged block is no attribute mutation.
    fn write_text(&self, before: &str, text: String) -> JsResult<()> {
        if text == before {
            return Ok(());
        }
        self.write_raw(text)
    }
    fn write_raw(&self, text: String) -> JsResult<()> {
        match &self.source {
            StyleSource::Inline(el) => el.page.dom.borrow_mut().set_attribute(el.id, "style", &text).js(),
            StyleSource::Rule { owner, node } => {
                if let RuleKind::Style { block, .. } = &node.kind {
                    *block.borrow_mut() = text;
                    commit_edit(&owner.page, owner.id, &sheet_model(&owner.page, owner.id));
                }
                Ok(())
            }
        }
    }
    /// The declarations of `text` that can change what `name` reads (those
    /// setting one of its longhands), as a block's text.
    fn relevant(text: &str, name: &str) -> Option<String> {
        let relevant: Vec<Declaration> =
            parse_declarations(text).into_iter().filter(|d| declarations::overlaps(&d.name, name)).collect();
        (!relevant.is_empty()).then(|| serialize_declarations(&relevant))
    }
    /// `getPropertyValue(name)`. Only the declarations that can change the
    /// answer are parsed.
    fn get(&self, name: &str) -> String {
        let text = self.text();
        if text.is_empty() {
            return String::new();
        }
        if unparsed(name) {
            return parse_declarations(&text).into_iter().find(|d| d.name == name).map(|d| d.value).unwrap_or_default();
        }
        Self::relevant(&text, name).map(|text| DeclarationBlock::parse(&text).value(name)).unwrap_or_default()
    }
    /// `getPropertyPriority(name) == "important"`.
    fn important(&self, name: &str) -> bool {
        let text = self.text();
        if unparsed(name) {
            return parse_declarations(&text).iter().any(|d| d.name == name && d.important);
        }
        Self::relevant(&text, name).is_some_and(|text| DeclarationBlock::parse(&text).important(name))
    }
    /// The declarations of `text` as written, but those of no property.
    fn written(text: &str) -> Vec<Declaration> {
        let mut decls = parse_declarations(text);
        decls.retain(|d| css_property(&d.name));
        decls
    }
    /// `setProperty(name, value, priority)`: an empty value removes the
    /// property; an invalid one, or a name that is no property, changes
    /// nothing. When no other declaration sets a longhand of `name`, the new
    /// declaration replaces its own in place and the others stay as written;
    /// otherwise the whole block is parsed, updated and serialized.
    fn set(&self, name: &str, value: &str, important: bool) -> JsResult<()> {
        let value = value.trim();
        if value.is_empty() {
            self.remove(name)?;
            return Ok(());
        }
        if !css_property(name) {
            return Ok(());
        }
        let text = self.text();
        if unparsed(name) {
            let mut decls = Self::written(&text);
            match decls.iter_mut().find(|d| d.name == name) {
                Some(d) => {
                    d.value = value.to_owned();
                    d.important = important;
                }
                None => decls.push(Declaration { name: name.to_owned(), value: value.to_owned(), important }),
            }
            return self.write_text(&text, serialize_declarations(&decls));
        }
        let decls = Self::written(&text);
        if decls.iter().any(|d| d.name != name && !unparsed(&d.name) && declarations::overlaps(&d.name, name)) {
            let mut block = Block::parse(&text);
            return match block.parsed.set(name, value, important) {
                Some(true) => self.write(&block),
                _ => Ok(()),
            };
        }
        let Some(declaration) = declarations::declaration_text(name, value, important) else { return Ok(()) };
        let mut out = Vec::with_capacity(decls.len() + 1);
        let mut replaced = false;
        for d in &decls {
            if d.name == name {
                if !replaced {
                    out.push(declaration.clone());
                }
                replaced = true;
            } else {
                out.push(serialize_declarations(std::slice::from_ref(d)));
            }
        }
        if !replaced {
            out.push(declaration);
        }
        self.write_text(&text, out.join(" "))
    }
    /// `removeProperty(name)`: the value it had.
    fn remove(&self, name: &str) -> JsResult<String> {
        let text = self.text();
        if !text.contains(':') {
            return Ok(String::new());
        }
        let mut decls = Self::written(&text);
        if unparsed(name) {
            let Some(i) = decls.iter().position(|d| d.name == name) else { return Ok(String::new()) };
            let old = decls.remove(i).value;
            self.write_text(&text, serialize_declarations(&decls))?;
            return Ok(old);
        }
        if !decls.iter().any(|d| declarations::overlaps(&d.name, name)) {
            return Ok(String::new());
        }
        if decls.iter().all(|d| d.name == name || unparsed(&d.name) || !declarations::overlaps(&d.name, name)) {
            let old = Self::relevant(&text, name).map(|t| DeclarationBlock::parse(&t).value(name)).unwrap_or_default();
            decls.retain(|d| d.name != name);
            self.write_text(&text, serialize_declarations(&decls))?;
            return Ok(old);
        }
        let mut block = Block::parse(&text);
        let old = block.parsed.value(name);
        if block.parsed.remove(name) {
            self.write(&block)?;
        }
        Ok(old)
    }
    /// `cssText = text`: the block parsed from `text`, serialized.
    fn set_css_text(&self, text: &str) -> JsResult<()> {
        self.write(&Block::parse(text))
    }
}

pub struct CssStyleDeclaration;
impl JsClass for CssStyleDeclaration {
    const NAME: &'static str = "CSSStyleDeclaration";
    type Data = StyleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "cssText",
            |s, st| Ok(s.string(&st.block().css_text())),
            |_s, st, v| st.set_css_text(&dom_string(v)?),
        );
        def.getter("length", |s, st| Ok(s.number(st.block().names().len() as f64)));
        def.getter("parentRule", |s, st| match &st.source {
            StyleSource::Inline(_) => Ok(s.null()),
            StyleSource::Rule { owner, node } => Ok(rule_wrapper(s, node, owner)?.value()),
        });
        def.accessor(
            "cssFloat",
            |s, st| Ok(s.string(&st.get("float"))),
            |_s, st, v| st.set("float", &dom_string(v)?, false),
        );
        def.method("item", 1, |s, st, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(s.string(st.block().names().get(i).map(String::as_str).unwrap_or("")))
        });
        def.indexed(|s, st, i| Ok(st.block().names().get(i as usize).map(|name| s.string(name))));
        def.method("getPropertyValue", 1, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.getPropertyValue")?);
            Ok(s.string(&st.get(&name)))
        });
        def.method("getPropertyPriority", 1, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.getPropertyPriority")?);
            Ok(s.string(if st.important(&name) { "important" } else { "" }))
        });
        def.method("setProperty", 2, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.setProperty")?);
            let value = null_to_empty(a.get(1))?;
            let prio = if a.get(2).is_undefined() { String::new() } else { dom_string(a.get(2))? };
            if !prio.is_empty() && !prio.eq_ignore_ascii_case("important") {
                return Ok(s.undefined());
            }
            st.set(&name, &value, !prio.is_empty())?;
            Ok(s.undefined())
        });
        def.method("removeProperty", 1, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.removeProperty")?);
            Ok(s.string(&st.remove(&name)?))
        });
        def.named(false, |s, st, name| {
            let Some(property) = named_property(name) else { return Ok(None) };
            Ok(Some(s.string(&st.get(&property))))
        });
        def.named_setter(|_s, st, name, v| {
            let Some(property) = named_property(name) else { return Ok(false) };
            let value = if v.is_null() { String::new() } else { dom_string(v)? };
            st.set(&property, &value, false)?;
            Ok(true)
        });
        def.property_names(|_s, st| Ok(st.block().names()));
    }
}

pub fn inline_style<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let base = page.node_identity(id);
    let key = IdentityKey(base.0 | (1 << 61) | (22 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = crate::target::page_of(s)?;
    s.wrap::<CssStyleDeclaration>(StyleData { source: StyleSource::Inline(NodeHold::new(&page, id)?) }, Some(key))
}

// ----- stylesheets ------------------------------------------------------------

/// A sheet's CSSOM rules, parsed from its text on first use. Holds no JS
/// references. `source` is the text the rules came from: a `<style>` whose text
/// no longer matches is reparsed, which drops any CSSOM edits, as HTML does.
pub struct SheetModel {
    source: RefCell<String>,
    edited: Cell<bool>,
    list: RuleList,
    namespaces: RefCell<Namespaces>,
    /// `Dom::version` at the last source check: unchanged means nothing to compare.
    checked: Cell<u64>,
}

impl SheetModel {
    /// CSSOM edits replaced the text the style engine reads for this sheet.
    pub(crate) fn edited(&self) -> bool {
        self.edited.get()
    }
}

#[derive(Default)]
struct RuleList {
    rules: RefCell<Vec<Rc<RuleNode>>>,
    /// Bumped on every change; the JS array is rebuilt when it differs.
    generation: Cell<u32>,
}

impl RuleList {
    fn replace(&self, rules: Vec<Rc<RuleNode>>) {
        *self.rules.borrow_mut() = rules;
        self.generation.set(self.generation.get().wrapping_add(1));
    }
}

enum RuleKind {
    Style { selector: RefCell<String>, block: RefCell<String> },
    Group { name: String, condition: String, list: RuleList },
    Other { rule_type: u16, text: String },
}

struct RuleNode {
    identity: IdentityKey,
    kind: RuleKind,
    parent: Weak<RuleNode>,
}

impl RuleNode {
    fn new(parsed: ParsedRule, parent: Weak<RuleNode>) -> Rc<RuleNode> {
        static NEXT_RULE: AtomicU64 = AtomicU64::new(1);
        let identity = IdentityKey(1 << 56, NEXT_RULE.fetch_add(1, Ordering::Relaxed));
        Rc::new_cyclic(|me| {
            let kind = match parsed {
                ParsedRule::Style { selector, block } => {
                    RuleKind::Style { selector: RefCell::new(selector), block: RefCell::new(block) }
                }
                ParsedRule::Group { name, condition, rules } => {
                    let list = RuleList::default();
                    list.replace(rules.into_iter().map(|r| RuleNode::new(r, me.clone())).collect());
                    RuleKind::Group { name, condition, list }
                }
                ParsedRule::Other { rule_type, text } => RuleKind::Other { rule_type, text },
            };
            RuleNode { identity, kind, parent }
        })
    }

    /// The text given to the style engine: blocks stay as written.
    fn source_text(&self) -> String {
        match &self.kind {
            RuleKind::Style { selector, block } => format!("{} {{{}}}", selector.borrow(), block.borrow()),
            RuleKind::Group { name, condition, list } => {
                let body = list.rules.borrow().iter().map(|r| r.source_text()).collect::<Vec<_>>().join("\n");
                format!("@{name} {condition} {{\n{body}\n}}")
            }
            RuleKind::Other { text, .. } => text.clone(),
        }
    }

    fn css_text(&self) -> String {
        match &self.kind {
            RuleKind::Style { selector, block } => {
                let decls = serialize_declarations(&parse_declarations(&block.borrow()));
                if decls.is_empty() {
                    format!("{} {{ }}", selector.borrow())
                } else {
                    format!("{} {{ {decls} }}", selector.borrow())
                }
            }
            RuleKind::Group { name, condition, list } => {
                let mut out = format!("@{name} {condition} {{\n");
                for rule in list.rules.borrow().iter() {
                    out.push_str("  ");
                    out.push_str(&rule.css_text());
                    out.push('\n');
                }
                out.push('}');
                out
            }
            RuleKind::Other { text, .. } => text.clone(),
        }
    }
}

fn sheet_source(page: &PageDom, owner: NodeId) -> String {
    let dom = page.dom.borrow();
    if dom.element(owner).is_ok_and(|e| *e.local_name() == *"style") {
        return dom.text_content(owner).ok().flatten().unwrap_or_default();
    }
    page.layout().style().sheet_text(owner).unwrap_or_default().to_owned()
}

fn parsed_nodes(text: &str, namespaces: &mut Namespaces) -> Vec<Rc<RuleNode>> {
    parse_rules(text, namespaces).into_iter().map(|r| RuleNode::new(r, Weak::new())).collect()
}

/// The sheet's rules, parsed or reparsed if its text changed.
fn sheet_model(page: &PageDom, owner: NodeId) -> Rc<SheetModel> {
    let version = page.dom.borrow().version();
    let existing = page.sheet_models.borrow().get(&owner).cloned();
    if let Some(model) = existing.as_ref().filter(|m| m.checked.get() == version) {
        return model.clone();
    }
    let source = sheet_source(page, owner);
    let model = existing.unwrap_or_else(|| {
        let model = Rc::new(SheetModel {
            // Never equal to a real source, so the first use parses.
            source: RefCell::new("\0".into()),
            edited: Cell::new(false),
            list: RuleList::default(),
            namespaces: RefCell::default(),
            checked: Cell::new(version),
        });
        page.sheet_models.borrow_mut().insert(owner, model.clone());
        model
    });
    model.checked.set(version);
    if *model.source.borrow() != source {
        let mut namespaces = Vec::new();
        model.list.replace(parsed_nodes(&source, &mut namespaces));
        *model.namespaces.borrow_mut() = namespaces;
        if model.edited.replace(false) {
            page.set_style_element_sheet(owner);
        }
        *model.source.borrow_mut() = source;
    }
    model
}

/// A CSSOM edit: the style engine reads the rules' text from now on.
fn commit_edit(page: &PageDom, owner: NodeId, model: &SheetModel) {
    let text = model.list.rules.borrow().iter().map(|r| r.source_text()).collect::<Vec<_>>().join("\n");
    page.set_stylesheet_text(owner, &text);
    model.edited.set(true);
    if !page.dom.borrow().element(owner).is_ok_and(|e| *e.local_name() == *"style") {
        *model.source.borrow_mut() = text;
    }
}

fn insert_into(
    list: &RuleList,
    namespaces: &Namespaces,
    parent: Weak<RuleNode>,
    rule: &str,
    index: usize,
) -> JsResult<()> {
    let mut scoped = namespaces.clone();
    let mut parsed = parse_rules(rule, &mut scoped);
    let invalid = || JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'."));
    if parsed.len() != 1 {
        return Err(invalid());
    }
    if index > list.rules.borrow().len() {
        return Err(JsError::dom("IndexSizeError", "The index provided is larger than the number of rules."));
    }
    let node = RuleNode::new(parsed.pop().ok_or_else(invalid)?, parent);
    let mut rules = list.rules.borrow().clone();
    rules.insert(index, node);
    list.replace(rules);
    Ok(())
}

fn delete_from(list: &RuleList, index: usize) -> JsResult<()> {
    let mut rules = list.rules.borrow().clone();
    if index >= rules.len() {
        return Err(JsError::dom("IndexSizeError", "The index provided is outside the rule list."));
    }
    rules.remove(index);
    list.replace(rules);
    Ok(())
}

const RULES: &str = "__lpCssRules";
const GENERATION: &str = "__lpCssRulesGeneration";

/// The list as a JS array kept on its holder, so `cssRules` stays the same live
/// object and rules keep their identity. Rebuilt only after a change.
fn rules_array<'s>(
    s: &Scope<'s>,
    holder: Option<Object<'s>>,
    list: &RuleList,
    owner: &NodeHold,
) -> JsResult<Value<'s>> {
    let generation = list.generation.get();
    let mut array = None;
    if let Some(holder) = holder {
        array = s.get_hidden(holder, RULES)?.and_then(|v| v.as_object());
        if array.is_some() {
            let current = s.get_hidden(holder, GENERATION)?.and_then(|v| v.to_number().ok());
            if current == Some(generation as f64) {
                return Ok(array.map_or(s.undefined(), |a| a.value()));
            }
        }
    }
    let array = match array {
        Some(array) => array,
        None => s.array(&[])?,
    };
    if let Some(holder) = holder {
        s.set_hidden(holder, RULES, array.value())?;
        s.set_hidden(holder, GENERATION, s.number(generation as f64))?;
    }
    let rules = list.rules.borrow().clone();
    array.set_atom("length", s.number(0.0))?;
    for (i, rule) in rules.iter().enumerate() {
        let wrapper = rule_wrapper(s, rule, owner)?;
        array.set_index(i as u32, wrapper.value())?;
    }
    Ok(array.value())
}

/// Keeps a list a script already holds current after an edit.
fn sync_rules(s: &Scope<'_>, key: IdentityKey, list: &RuleList, owner: &NodeHold) -> JsResult<()> {
    let Some(holder) = s.lookup_identity(key) else { return Ok(()) };
    if s.get_hidden(holder, RULES)?.is_some() {
        rules_array(s, Some(holder), list, owner)?;
    }
    Ok(())
}

fn rule_wrapper<'s>(s: &Scope<'s>, rule: &Rc<RuleNode>, owner: &NodeHold) -> JsResult<Object<'s>> {
    if let Some(o) = s.lookup_identity(rule.identity) {
        return Ok(o);
    }
    let page = owner.page.clone();
    let data = RuleData { owner: NodeHold::new(&page, owner.id)?, node: rule.clone() };
    match rule.kind {
        RuleKind::Style { .. } => s.wrap::<CssStyleRule>(data, Some(rule.identity)),
        RuleKind::Group { .. } => s.wrap::<CssGroupRule>(data, Some(rule.identity)),
        RuleKind::Other { .. } => s.wrap::<CssOtherRule>(data, Some(rule.identity)),
    }
}

fn sheet_key(page: &PageDom, owner: NodeId) -> IdentityKey {
    let base = page.node_identity(owner);
    IdentityKey(base.0 | (1 << 61) | (23 << 48), base.1)
}

pub struct RuleData {
    owner: NodeHold,
    node: Rc<RuleNode>,
}

fn define_rule<C: JsClass<Data = RuleData>>(def: &mut ClassDef<C>) {
    def.getter("type", |s, r| {
        Ok(s.number(match &r.node.kind {
            RuleKind::Style { .. } => 1.0,
            RuleKind::Group { name, .. } if name == "media" => 4.0,
            RuleKind::Group { .. } => 12.0,
            RuleKind::Other { rule_type, .. } => f64::from(*rule_type),
        }))
    });
    def.getter("cssText", |s, r| Ok(s.string(&r.node.css_text())));
    def.getter("parentRule", |s, r| match r.node.parent.upgrade() {
        Some(parent) => Ok(rule_wrapper(s, &parent, &r.owner)?.value()),
        None => Ok(s.null()),
    });
    def.getter("parentStyleSheet", |s, r| Ok(sheet_wrapper(s, &r.owner.page, r.owner.id)?.value()));
}

pub struct CssStyleRule;
impl JsClass for CssStyleRule {
    const NAME: &'static str = "CSSStyleRule";
    type Data = RuleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_rule(def);
        // The rule's declaration block ([PutForwards=cssText]); one object
        // per rule and realm.
        def.accessor(
            "style",
            |s, r| {
                let key = IdentityKey(r.node.identity.0 | (1 << 55), r.node.identity.1);
                if let Some(o) = s.lookup_identity(key) {
                    return Ok(o.value());
                }
                let source =
                    StyleSource::Rule { owner: NodeHold::new(&r.owner.page, r.owner.id)?, node: r.node.clone() };
                Ok(s.wrap::<CssStyleDeclaration>(StyleData { source }, Some(key))?.value())
            },
            |_s, r, v| {
                let source =
                    StyleSource::Rule { owner: NodeHold::new(&r.owner.page, r.owner.id)?, node: r.node.clone() };
                StyleData { source }.set_css_text(&dom_string(v)?)
            },
        );
        def.accessor(
            "selectorText",
            |s, r| match &r.node.kind {
                RuleKind::Style { selector, .. } => Ok(s.string(&selector.borrow())),
                _ => Ok(s.undefined()),
            },
            |_s, r, v| {
                let text = dom_string(v)?;
                let model = sheet_model(&r.owner.page, r.owner.id);
                let normalized = normalize_selector(&text, &model.namespaces.borrow());
                // An invalid selector leaves the rule unchanged.
                if let (Some(normalized), RuleKind::Style { selector, .. }) = (normalized, &r.node.kind) {
                    *selector.borrow_mut() = normalized;
                    commit_edit(&r.owner.page, r.owner.id, &model);
                }
                Ok(())
            },
        );
    }
}

pub struct CssGroupRule;
impl JsClass for CssGroupRule {
    const NAME: &'static str = "CSSGroupingRule";
    type Data = RuleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_rule(def);
        def.getter("conditionText", |s, r| match &r.node.kind {
            RuleKind::Group { condition, .. } => Ok(s.string(condition)),
            _ => Ok(s.undefined()),
        });
        def.getter("cssRules", |s, r| {
            let RuleKind::Group { list, .. } = &r.node.kind else { return Ok(s.undefined()) };
            rules_array(s, s.lookup_identity(r.node.identity), list, &r.owner)
        });
        def.method("insertRule", 1, |s, r, a| {
            let rule = arg_string(&a, 0, "CSSGroupingRule.insertRule")?;
            let index = if a.get(1).is_undefined() { 0 } else { to_u32(a.get(1))? as usize };
            let RuleKind::Group { list, .. } = &r.node.kind else { return Ok(s.undefined()) };
            let model = sheet_model(&r.owner.page, r.owner.id);
            let namespaces = model.namespaces.borrow().clone();
            insert_into(list, &namespaces, Rc::downgrade(&r.node), &rule, index)?;
            commit_edit(&r.owner.page, r.owner.id, &model);
            sync_rules(s, r.node.identity, list, &r.owner)?;
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, r, a| {
            let index = to_u32(a.get(0))? as usize;
            let RuleKind::Group { list, .. } = &r.node.kind else { return Ok(s.undefined()) };
            delete_from(list, index)?;
            commit_edit(&r.owner.page, r.owner.id, &sheet_model(&r.owner.page, r.owner.id));
            sync_rules(s, r.node.identity, list, &r.owner)?;
            Ok(s.undefined())
        });
    }
}

pub struct CssOtherRule;
impl JsClass for CssOtherRule {
    const NAME: &'static str = "CSSRule";
    type Data = RuleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_rule(def);
    }
}

pub struct SheetData {
    owner: NodeHold,
}

pub struct CssStyleSheet;
impl JsClass for CssStyleSheet {
    const NAME: &'static str = "CSSStyleSheet";
    type Data = SheetData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("ownerNode", |s, sh| Ok(crate::node::wrap_node(s, &sh.owner.page, sh.owner.id)?.value()));
        def.getter("type", |s, _sh| Ok(s.string("text/css")));
        def.getter("href", |s, sh| {
            let dom = sh.owner.page.dom.borrow();
            Ok(match dom.get_attribute(sh.owner.id, "href").js()? {
                Some(h) => s.string(&crate::html::resolve_url(&dom, sh.owner.id, h)),
                None => s.null(),
            })
        });
        def.getter("disabled", |s, _sh| Ok(s.boolean(false)));
        def.getter("cssRules", rules_value);
        def.getter("rules", rules_value);
        def.method("insertRule", 1, |s, sh, a| {
            let rule = arg_string(&a, 0, "CSSStyleSheet.insertRule")?;
            let index = if a.get(1).is_undefined() { 0 } else { to_u32(a.get(1))? as usize };
            let model = sheet_model(&sh.owner.page, sh.owner.id);
            let namespaces = model.namespaces.borrow().clone();
            insert_into(&model.list, &namespaces, Weak::new(), &rule, index)?;
            commit_edit(&sh.owner.page, sh.owner.id, &model);
            sync_rules(s, sheet_key(&sh.owner.page, sh.owner.id), &model.list, &sh.owner)?;
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, sh, a| {
            let index = to_u32(a.get(0))? as usize;
            let model = sheet_model(&sh.owner.page, sh.owner.id);
            delete_from(&model.list, index)?;
            commit_edit(&sh.owner.page, sh.owner.id, &model);
            sync_rules(s, sheet_key(&sh.owner.page, sh.owner.id), &model.list, &sh.owner)?;
            Ok(s.undefined())
        });
        def.method("replaceSync", 1, |s, sh, a| {
            let text = arg_string(&a, 0, "CSSStyleSheet.replaceSync")?;
            let model = sheet_model(&sh.owner.page, sh.owner.id);
            let mut namespaces = Vec::new();
            model.list.replace(parsed_nodes(&text, &mut namespaces));
            *model.namespaces.borrow_mut() = namespaces;
            commit_edit(&sh.owner.page, sh.owner.id, &model);
            sync_rules(s, sheet_key(&sh.owner.page, sh.owner.id), &model.list, &sh.owner)?;
            Ok(s.undefined())
        });
    }
}

fn rules_value<'s>(s: &Scope<'s>, sh: &SheetData) -> JsResult<Value<'s>> {
    let model = sheet_model(&sh.owner.page, sh.owner.id);
    let key = sheet_key(&sh.owner.page, sh.owner.id);
    rules_array(s, s.lookup_identity(key), &model.list, &sh.owner)
}

fn sheet_wrapper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, owner: NodeId) -> JsResult<Object<'s>> {
    let key = sheet_key(page, owner);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<CssStyleSheet>(SheetData { owner: NodeHold::new(page, owner)? }, Some(key))
}

pub(crate) fn element_sheet<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let connected = n.page().dom.borrow().is_connected(n.id).js()?;
    if !connected {
        return Ok(s.null());
    }
    sheet_object(s, n.page(), n.id)
}

/// The CSSStyleSheet of the connected `<style>` or `<link>` element `owner`.
fn sheet_object<'s>(s: &Scope<'s>, page: &Rc<PageDom>, owner: NodeId) -> JsResult<Value<'s>> {
    let base = page.node_identity(owner);
    let key = IdentityKey(base.0 | (1 << 61) | (23 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<CssStyleSheet>(SheetData { owner: NodeHold::new(page, owner)? }, Some(key))?.value())
}

/// `document.styleSheets`: the sheets of the document's `<style>` elements
/// and stylesheet links, in tree order. Live: it reads the tree each time.
pub struct StyleSheetList;

impl StyleSheetList {
    fn owners(root: &NodeHold) -> Vec<NodeId> {
        let dom = root.page.dom.borrow();
        let Ok(nodes) = dom.descendants(root.id) else { return Vec::new() };
        nodes
            .filter(|&n| {
                dom.element(n).is_ok_and(|e| match &*e.name.local {
                    "style" => e.is_html() || e.namespace() == &markup5ever::ns!(svg),
                    "link" => {
                        e.is_html()
                            && e.attr("rel").is_some_and(|rel| {
                                rel.split_ascii_whitespace().any(|t| t.eq_ignore_ascii_case("stylesheet"))
                            })
                    }
                    _ => false,
                })
            })
            .collect()
    }
}

impl JsClass for StyleSheetList {
    const NAME: &'static str = "StyleSheetList";
    type Data = NodeHold;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, root| Ok(s.number(StyleSheetList::owners(root).len() as f64)));
        def.method("item", 1, |s, root, a| match StyleSheetList::owners(root).get(to_u32(a.get(0))? as usize) {
            Some(&owner) => sheet_object(s, &root.page, owner),
            None => Ok(s.null()),
        });
        def.indexed(|s, root, i| match StyleSheetList::owners(root).get(i as usize) {
            Some(&owner) => sheet_object(s, &root.page, owner).map(Some),
            None => Ok(None),
        });
        def.indexed_iterator();
    }
}

/// `document.styleSheets` (one list object per document and realm).
pub(crate) fn style_sheets<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let base = n.page().node_identity(n.id);
    let key = IdentityKey(base.0 | (1 << 61) | (24 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<StyleSheetList>(NodeHold::new(n.page(), n.id)?, Some(key))?.value())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CssStyleDeclaration>();
    engine.register::<CssStyleSheet>();
    engine.register::<StyleSheetList>();
    engine.register::<CssStyleRule>();
    engine.register::<CssGroupRule>();
    engine.register::<CssOtherRule>();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn declarations() {
        let d = parse_declarations("color: red; background: url(\"a;b.png\") ; --x: 1 ;display:none !important");
        assert_eq!(d.len(), 4);
        assert_eq!(d[1].value, "url(\"a;b.png\")");
        assert!(d[3].important);
        assert_eq!(camel_to_css("backgroundColor"), "background-color");
        assert_eq!(camel_to_css("webkitTransform"), "-webkit-transform");
        assert_eq!(serialize_declarations(&d[..1]), "color: red;");
    }
}
