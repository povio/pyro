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
/// Sorted (binary search).
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
    "-webkit-text-size-adjust",
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
    "column-fill",
    "column-rule",
    "column-rule-color",
    "column-rule-style",
    "column-rule-width",
    "contain-intrinsic-height",
    "contain-intrinsic-size",
    "contain-intrinsic-width",
    "content-visibility",
    "counter-set",
    "dominant-baseline",
    "field-sizing",
    "flood-color",
    "flood-opacity",
    "font-synthesis",
    "forced-color-adjust",
    "hyphens",
    "image-orientation",
    "initial-letter",
    "interpolate-size",
    "lighting-color",
    "line-clamp",
    "marker",
    "marker-end",
    "marker-mid",
    "marker-start",
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
    "stop-color",
    "stop-opacity",
    "tab-size",
    "text-anchor",
    "text-combine-upright",
    "text-emphasis",
    "text-emphasis-color",
    "text-emphasis-position",
    "text-emphasis-style",
    "text-orientation",
    "text-size-adjust",
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
    // The webkit-cased attributes (`WebkitTransform`) as the lowercase ones.
    if let Some(rest) = name.strip_prefix("Webkit").filter(|r| r.starts_with(|c: char| c.is_ascii_uppercase())) {
        return named_property(&format!("webkit{rest}"));
    }
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
    Rule { owner: SheetOwner, node: Rc<RuleNode> },
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
                    owner.commit(&owner.model());
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
                // As the rule's `style.cssText` serializes the block.
                let decls = Block::parse(&block.borrow()).css_text();
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
    constructed: bool,
) -> JsResult<()> {
    let mut scoped = namespaces.clone();
    let mut parsed = parse_rules(rule, &mut scoped);
    let invalid = || JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'."));
    if parsed.len() != 1 {
        return Err(invalid());
    }
    // Constructed sheets take no `@import` (CSSOM's insert a CSS rule).
    if constructed && matches!(parsed[0], ParsedRule::Other { rule_type: IMPORT_RULE, .. }) {
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
    owner: &SheetOwner,
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
fn sync_rules(s: &Scope<'_>, key: IdentityKey, list: &RuleList, owner: &SheetOwner) -> JsResult<()> {
    let Some(holder) = s.lookup_identity(key) else { return Ok(()) };
    if s.get_hidden(holder, RULES)?.is_some() {
        rules_array(s, Some(holder), list, owner)?;
    }
    Ok(())
}

fn rule_wrapper<'s>(s: &Scope<'s>, rule: &Rc<RuleNode>, owner: &SheetOwner) -> JsResult<Object<'s>> {
    if let Some(o) = s.lookup_identity(rule.identity) {
        return Ok(o);
    }
    let data = RuleData { owner: owner.hold()?, node: rule.clone() };
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
    owner: SheetOwner,
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
    def.getter("parentStyleSheet", |s, r| Ok(r.owner.wrapper(s)?.value()));
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
                let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node.clone() };
                Ok(s.wrap::<CssStyleDeclaration>(StyleData { source }, Some(key))?.value())
            },
            |_s, r, v| {
                let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node.clone() };
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
                let model = r.owner.model();
                let normalized = normalize_selector(&text, &model.namespaces.borrow());
                // An invalid selector leaves the rule unchanged.
                if let (Some(normalized), RuleKind::Style { selector, .. }) = (normalized, &r.node.kind) {
                    *selector.borrow_mut() = normalized;
                    r.owner.commit(&model);
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
            let model = r.owner.model();
            let namespaces = model.namespaces.borrow().clone();
            insert_into(list, &namespaces, Rc::downgrade(&r.node), &rule, index, r.owner.is_constructed())?;
            r.owner.commit(&model);
            sync_rules(s, r.node.identity, list, &r.owner)?;
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, r, a| {
            let index = to_u32(a.get(0))? as usize;
            let RuleKind::Group { list, .. } = &r.node.kind else { return Ok(s.undefined()) };
            delete_from(list, index)?;
            r.owner.commit(&r.owner.model());
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

/// `CSSRule.type` of `@import`.
const IMPORT_RULE: u16 = 3;

/// A sheet made by `new CSSStyleSheet()`: it has no owner node and applies
/// where a document or shadow root adopts it (`adoptedStyleSheets`).
pub struct Constructed {
    /// Its id in the style engine's adopted sheets.
    id: u64,
    page: Weak<PageDom>,
    /// Its constructor document: only that document and its shadow roots
    /// adopt it.
    document: NodeId,
    identity: IdentityKey,
    model: Rc<SheetModel>,
    /// Its media list's text: the rules apply inside this `@media`.
    media: RefCell<String>,
    disabled: Cell<bool>,
}

impl Constructed {
    fn new(page: &Rc<PageDom>, media: String, disabled: bool) -> Rc<Constructed> {
        static NEXT_SHEET: AtomicU64 = AtomicU64::new(1);
        let id = NEXT_SHEET.fetch_add(1, Ordering::Relaxed);
        Rc::new(Constructed {
            id,
            page: Rc::downgrade(page),
            document: page.document(),
            identity: IdentityKey((1 << 56) | (1 << 52), id),
            model: Rc::new(SheetModel {
                source: RefCell::default(),
                edited: Cell::new(false),
                list: RuleList::default(),
                namespaces: RefCell::default(),
                checked: Cell::new(0),
            }),
            media: RefCell::new(media_text(&media)),
            disabled: Cell::new(disabled),
        })
    }

    /// The text the style engine applies: the rules (inside the sheet's
    /// `@media`), nothing while disabled.
    fn applied_text(&self) -> Rc<str> {
        let source = self.model.source.borrow();
        let media = self.media.borrow();
        if self.disabled.get() || source.is_empty() {
            "".into()
        } else if media.is_empty() {
            source.as_str().into()
        } else {
            format!("@media {media} {{\n{source}\n}}").into()
        }
    }

    /// The rules changed: adopters restyle.
    fn commit(&self) {
        let text = self.model.list.rules.borrow().iter().map(|r| r.source_text()).collect::<Vec<_>>().join("\n");
        *self.model.source.borrow_mut() = text;
        self.push();
    }

    fn push(&self) {
        if let Some(page) = self.page.upgrade() {
            page.set_constructed_sheet_text(self.id, &self.applied_text());
        }
    }

    /// `replace(text)`/`replaceSync(text)`: the rules of `text`, but
    /// `@import`s, which constructed sheets do not load.
    fn replace(&self, text: &str) {
        let mut namespaces = Vec::new();
        let rules = parse_rules(text, &mut namespaces)
            .into_iter()
            .filter(|r| !matches!(r, ParsedRule::Other { rule_type: IMPORT_RULE, .. }))
            .map(|r| RuleNode::new(r, Weak::new()))
            .collect();
        self.model.list.replace(rules);
        *self.model.namespaces.borrow_mut() = namespaces;
        self.commit();
    }
}

/// Whose sheet a CSSOM object belongs to.
pub enum SheetOwner {
    /// A `<style>`'s or `<link>`'s.
    Node(NodeHold),
    Constructed(Rc<Constructed>),
}

impl SheetOwner {
    /// Another reference to the same sheet.
    fn hold(&self) -> JsResult<SheetOwner> {
        Ok(match self {
            SheetOwner::Node(h) => SheetOwner::Node(NodeHold::new(&h.page, h.id)?),
            SheetOwner::Constructed(c) => SheetOwner::Constructed(c.clone()),
        })
    }
    fn model(&self) -> Rc<SheetModel> {
        match self {
            SheetOwner::Node(h) => sheet_model(&h.page, h.id),
            SheetOwner::Constructed(c) => c.model.clone(),
        }
    }
    /// A CSSOM edit of `model`'s rules.
    fn commit(&self, model: &SheetModel) {
        match self {
            SheetOwner::Node(h) => commit_edit(&h.page, h.id, model),
            SheetOwner::Constructed(c) => c.commit(),
        }
    }
    fn key(&self) -> IdentityKey {
        match self {
            SheetOwner::Node(h) => sheet_key(&h.page, h.id),
            SheetOwner::Constructed(c) => c.identity,
        }
    }
    fn is_constructed(&self) -> bool {
        matches!(self, SheetOwner::Constructed(_))
    }
    fn wrapper<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        match self {
            SheetOwner::Node(h) => sheet_wrapper(s, &h.page, h.id),
            SheetOwner::Constructed(c) => constructed_wrapper(s, c),
        }
    }
}

/// The media queries of a media list's text (split at top-level commas).
fn media_queries(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0);
    for (i, c) in text.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth <= 0 => {
                out.push(text[start..i].split_ascii_whitespace().collect::<Vec<_>>().join(" "));
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(text[start..].split_ascii_whitespace().collect::<Vec<_>>().join(" "));
    out.retain(|q| !q.is_empty());
    out
}

/// A media list's text as `mediaText` serializes it.
fn media_text(text: &str) -> String {
    media_queries(text).join(", ")
}

/// `CSSStyleSheet.media`: the `media` attribute of a sheet's owner, or a
/// constructed sheet's list.
pub struct MediaList;
impl MediaList {
    fn text(sh: &SheetOwner) -> String {
        match sh {
            SheetOwner::Node(h) => {
                media_text(h.page.dom.borrow().get_attribute(h.id, "media").ok().flatten().unwrap_or(""))
            }
            SheetOwner::Constructed(c) => c.media.borrow().clone(),
        }
    }
    fn set(sh: &SheetOwner, text: &str) -> JsResult<()> {
        let text = media_text(text);
        match sh {
            SheetOwner::Node(h) => h.page.dom.borrow_mut().set_attribute(h.id, "media", &text).js(),
            SheetOwner::Constructed(c) => {
                if c.media.replace(text.clone()) != text {
                    c.push();
                }
                Ok(())
            }
        }
    }
}

impl JsClass for MediaList {
    const NAME: &'static str = "MediaList";
    type Data = SheetOwner;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "mediaText",
            |s, sh| Ok(s.string(&MediaList::text(sh))),
            |_s, sh, v| MediaList::set(sh, &null_to_empty(v)?),
        );
        def.getter("length", |s, sh| Ok(s.number(media_queries(&MediaList::text(sh)).len() as f64)));
        def.method("item", 1, |s, sh, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(media_queries(&MediaList::text(sh)).get(i).map_or(s.null(), |q| s.string(q)))
        });
        def.indexed(|s, sh, i| Ok(media_queries(&MediaList::text(sh)).get(i as usize).map(|q| s.string(q))));
        def.method("appendMedium", 1, |s, sh, a| {
            let medium = media_text(&arg_string(&a, 0, "MediaList.appendMedium")?);
            let mut queries = media_queries(&MediaList::text(sh));
            if !medium.is_empty() && !queries.contains(&medium) {
                queries.push(medium);
                MediaList::set(sh, &queries.join(", "))?;
            }
            Ok(s.undefined())
        });
        def.method("deleteMedium", 1, |s, sh, a| {
            let medium = media_text(&arg_string(&a, 0, "MediaList.deleteMedium")?);
            let mut queries = media_queries(&MediaList::text(sh));
            let before = queries.len();
            queries.retain(|q| *q != medium);
            if queries.len() == before {
                return Err(JsError::dom("NotFoundError", "Failed to delete the medium: not in the list."));
            }
            MediaList::set(sh, &queries.join(", "))?;
            Ok(s.undefined())
        });
        def.method("toString", 0, |s, sh, _a| Ok(s.string(&MediaList::text(sh))));
    }
}

/// The `default` export of a CSS module script (HTML "create a CSS module
/// script"): a constructed sheet of the realm's document holding `text`'s
/// rules, as `new CSSStyleSheet()` + `replaceSync(text)` makes it.
pub fn module_sheet<'s>(s: &Scope<'s>, text: &str) -> JsResult<Object<'s>> {
    let page = crate::target::page_of(s)?;
    let sheet = Constructed::new(&page, String::new(), false);
    sheet.replace(text);
    let key = sheet.identity;
    s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(sheet) }, Some(key))
}

fn constructed_wrapper<'s>(s: &Scope<'s>, c: &Rc<Constructed>) -> JsResult<Object<'s>> {
    if let Some(o) = s.lookup_identity(c.identity) {
        return Ok(o);
    }
    s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(c.clone()) }, Some(c.identity))
}

pub struct SheetData {
    owner: SheetOwner,
}

fn not_constructed(what: &str) -> JsError {
    JsError::dom(
        "NotAllowedError",
        format!("Failed to execute '{what}' on 'CSSStyleSheet': Can't call {what} on non-constructed CSSStyleSheets."),
    )
}

/// The `{media, disabled}` of `new CSSStyleSheet(options)`.
fn sheet_options(a: Value<'_>) -> JsResult<(String, bool)> {
    if a.is_undefined() || a.is_null() {
        return Ok((String::new(), false));
    }
    let Some(options) = a.as_object() else {
        return Err(JsError::Type(
            "Failed to construct 'CSSStyleSheet': The provided value is not of type 'CSSStyleSheetInit'.".into(),
        ));
    };
    let media = options.get("media")?;
    let media = if media.is_undefined() {
        String::new()
    } else if let Some(list) = media.as_object().filter(|o| !o.is_function()) {
        // A MediaList: its mediaText.
        let text = list.get("mediaText")?;
        if text.is_undefined() {
            dom_string(media)?
        } else {
            dom_string(text)?
        }
    } else {
        dom_string(media)?
    };
    let disabled = options.get("disabled")?.to_bool();
    Ok((media.trim().to_owned(), disabled))
}

pub struct CssStyleSheet;
impl JsClass for CssStyleSheet {
    const NAME: &'static str = "CSSStyleSheet";
    type Data = SheetData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let (media, disabled) = sheet_options(a.get(0))?;
            let page = crate::target::page_of(s)?;
            let sheet = Constructed::new(&page, media, disabled);
            let key = sheet.identity;
            s.wrap_new::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(sheet) }, nt, Some(key))
        });
        def.getter("ownerNode", |s, sh| match &sh.owner {
            SheetOwner::Node(h) => Ok(crate::node::wrap_node(s, &h.page, h.id)?.value()),
            SheetOwner::Constructed(_) => Ok(s.null()),
        });
        def.getter("ownerRule", |s, _sh| Ok(s.null()));
        def.getter("parentStyleSheet", |s, _sh| Ok(s.null()));
        def.getter("title", |s, sh| match &sh.owner {
            SheetOwner::Node(h) => {
                let dom = h.page.dom.borrow();
                Ok(dom.get_attribute(h.id, "title").js()?.filter(|t| !t.is_empty()).map_or(s.null(), |t| s.string(t)))
            }
            SheetOwner::Constructed(_) => Ok(s.null()),
        });
        def.getter("type", |s, _sh| Ok(s.string("text/css")));
        // [SameObject, PutForwards=mediaText].
        def.accessor(
            "media",
            |s, sh| {
                let key = match &sh.owner {
                    SheetOwner::Node(h) => {
                        let base = h.page.node_identity(h.id);
                        IdentityKey(base.0 | (1 << 61) | (25 << 48), base.1)
                    }
                    SheetOwner::Constructed(c) => IdentityKey((1 << 56) | (1 << 53), c.id),
                };
                if let Some(o) = s.lookup_identity(key) {
                    return Ok(o.value());
                }
                Ok(s.wrap::<MediaList>(sh.owner.hold()?, Some(key))?.value())
            },
            |_s, sh, v| MediaList::set(&sh.owner, &dom_string(v)?),
        );
        def.getter("href", |s, sh| match &sh.owner {
            SheetOwner::Node(h) => {
                let dom = h.page.dom.borrow();
                Ok(match dom.get_attribute(h.id, "href").js()? {
                    Some(href) => s.string(&crate::html::resolve_url(&dom, h.id, href)),
                    None => s.null(),
                })
            }
            // A constructed sheet's location is its document's URL.
            SheetOwner::Constructed(c) => match c.page.upgrade() {
                Some(page) => {
                    let dom = page.dom.borrow();
                    Ok(s.string(&crate::html::resolve_url(&dom, c.document, "")))
                }
                None => Ok(s.null()),
            },
        });
        def.accessor(
            "disabled",
            |s, sh| Ok(s.boolean(matches!(&sh.owner, SheetOwner::Constructed(c) if c.disabled.get()))),
            |_s, sh, v| {
                if let SheetOwner::Constructed(c) = &sh.owner {
                    if c.disabled.replace(v.to_bool()) != v.to_bool() {
                        c.push();
                    }
                }
                Ok(())
            },
        );
        def.getter("cssRules", rules_value);
        def.getter("rules", rules_value);
        def.method("insertRule", 1, |s, sh, a| {
            let rule = arg_string(&a, 0, "CSSStyleSheet.insertRule")?;
            let index = if a.get(1).is_undefined() { 0 } else { to_u32(a.get(1))? as usize };
            let model = sh.owner.model();
            let namespaces = model.namespaces.borrow().clone();
            insert_into(&model.list, &namespaces, Weak::new(), &rule, index, sh.owner.is_constructed())?;
            sh.owner.commit(&model);
            sync_rules(s, sh.owner.key(), &model.list, &sh.owner)?;
            Ok(s.number(index as f64))
        });
        def.method("addRule", 0, |s, sh, a| {
            // Legacy: `addRule(selector, style, index)` inserts
            // `selector { style }` at `index` (the end by default); -1.
            let selector = if a.get(0).is_undefined() { "undefined".to_owned() } else { dom_string(a.get(0))? };
            let block = if a.get(1).is_undefined() { "undefined".to_owned() } else { dom_string(a.get(1))? };
            let model = sh.owner.model();
            let index =
                if a.get(2).is_undefined() { model.list.rules.borrow().len() } else { to_u32(a.get(2))? as usize };
            let namespaces = model.namespaces.borrow().clone();
            let rule = format!("{selector} {{ {block} }}");
            insert_into(&model.list, &namespaces, Weak::new(), &rule, index, sh.owner.is_constructed())?;
            sh.owner.commit(&model);
            sync_rules(s, sh.owner.key(), &model.list, &sh.owner)?;
            Ok(s.number(-1.0))
        });
        def.method("deleteRule", 1, |s, sh, a| {
            let index = to_u32(a.get(0))? as usize;
            let model = sh.owner.model();
            delete_from(&model.list, index)?;
            sh.owner.commit(&model);
            sync_rules(s, sh.owner.key(), &model.list, &sh.owner)?;
            Ok(s.undefined())
        });
        def.method("removeRule", 0, |s, sh, a| {
            let index = if a.get(0).is_undefined() { 0 } else { to_u32(a.get(0))? as usize };
            let model = sh.owner.model();
            delete_from(&model.list, index)?;
            sh.owner.commit(&model);
            sync_rules(s, sh.owner.key(), &model.list, &sh.owner)?;
            Ok(s.undefined())
        });
        def.method("replaceSync", 1, |s, sh, a| {
            let text = arg_string(&a, 0, "CSSStyleSheet.replaceSync")?;
            let SheetOwner::Constructed(c) = &sh.owner else { return Err(not_constructed("replaceSync")) };
            c.replace(&text);
            sync_rules(s, c.identity, &c.model.list, &sh.owner)?;
            Ok(s.undefined())
        });
        def.method("replace", 1, |s, sh, a| {
            let deferred = s.deferred()?;
            let promise = deferred.promise.get(s);
            let text = arg_string(&a, 0, "CSSStyleSheet.replace")?;
            match &sh.owner {
                SheetOwner::Constructed(c) => {
                    c.replace(&text);
                    sync_rules(s, c.identity, &c.model.list, &sh.owner)?;
                    deferred.resolve(s, constructed_wrapper(s, c)?.value())?;
                }
                SheetOwner::Node(_) => deferred.reject(s, not_constructed("replace").to_value(s))?,
            }
            Ok(promise)
        });
    }
}

fn rules_value<'s>(s: &Scope<'s>, sh: &SheetData) -> JsResult<Value<'s>> {
    let model = sh.owner.model();
    rules_array(s, s.lookup_identity(sh.owner.key()), &model.list, &sh.owner)
}

fn sheet_wrapper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, owner: NodeId) -> JsResult<Object<'s>> {
    let key = sheet_key(page, owner);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Node(NodeHold::new(page, owner)?) }, Some(key))
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
    Ok(s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Node(NodeHold::new(page, owner)?) }, Some(key))?.value())
}

/// `document.styleSheets`: the sheets of the document's `<style>` elements
/// and stylesheet links, in tree order. Live: it reads the tree each time.
pub struct StyleSheetList;

impl StyleSheetList {
    fn owners(root: &NodeHold) -> Vec<NodeId> {
        let dom = root.page.dom.borrow();
        // A disconnected shadow root's `<style>`s have no sheets.
        if !dom.is_connected(root.id).unwrap_or(false) {
            return Vec::new();
        }
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

/// `document.styleSheets` / `shadowRoot.styleSheets` (one list object per
/// document or shadow root and realm).
pub(crate) fn style_sheets<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let base = n.page().node_identity(n.id);
    let key = IdentityKey(base.0 | (1 << 61) | (24 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<StyleSheetList>(NodeHold::new(n.page(), n.id)?, Some(key))?.value())
}

// ----- adoptedStyleSheets -----------------------------------------------------

/// Hidden on a document's or shadow root's wrapper: its `adoptedStyleSheets`
/// array (a proxy) and the array behind it.
const ADOPTED: &str = "__lpAdoptedSheets";
const ADOPTED_TARGET: &str = "__lpAdoptedSheetsTarget";
/// Hidden on `navigator`: the realm's proxy factory, made on first use.
const ADOPTED_FACTORY: &str = "lpAdoptedSheetsArray";

/// The `ObservableArray<CSSStyleSheet>` behaviour of `adoptedStyleSheets`
/// over a plain array: writes are checked (`check`) and every change
/// adopts the array's sheets (`sync`).
const ADOPTED_JS: &str = r#"(function (target, check, sync) {
  'use strict';
  const index = (k) => typeof k === 'string' && String(k >>> 0) === k && (k >>> 0) !== 4294967295;
  return new Proxy(target, {
    set(t, k, v, r) {
      if (index(k)) {
        if (+k > t.length) return false;
        check(v);
        t[k] = v;
        sync(t);
        return true;
      }
      if (k === 'length') {
        const ok = Reflect.set(t, k, v);
        sync(t);
        return ok;
      }
      return Reflect.set(t, k, v, r);
    },
    defineProperty(t, k, d) {
      if (index(k)) {
        if (+k > t.length || !('value' in d)) return false;
        check(d.value);
        t[k] = d.value;
        sync(t);
        return true;
      }
      const ok = Reflect.defineProperty(t, k, d);
      if (k === 'length') sync(t);
      return ok;
    },
    deleteProperty(t, k) {
      if (index(k)) {
        if (+k >= t.length) return true;
        if (+k !== t.length - 1) return false;
        t.length = t.length - 1;
        sync(t);
        return true;
      }
      return Reflect.deleteProperty(t, k);
    },
  });
})"#;

/// The constructed sheet `v` is, if `scope` (a document or shadow root) may
/// adopt it.
fn adoptable(s: &Scope<'_>, page: &PageDom, scope: NodeId, v: Value<'_>) -> JsResult<Rc<Constructed>> {
    let Some(sheet) = s.unwrap::<CssStyleSheet>(v) else {
        return Err(JsError::Type(
            "Failed to set the 'adoptedStyleSheets' property: Failed to convert value to 'CSSStyleSheet'.".into(),
        ));
    };
    let SheetOwner::Constructed(c) = &sheet.owner else {
        return Err(JsError::dom("NotAllowedError", "Can't adopt non-constructed stylesheets."));
    };
    let document = page.dom.borrow().node_document(scope).js()?;
    if c.document != document {
        return Err(JsError::dom(
            "NotAllowedError",
            "Sharing constructed stylesheets in multiple documents is not allowed",
        ));
    }
    Ok(c.clone())
}

/// `scope` adopts `sheets`, in order.
fn adopt(page: &PageDom, scope: NodeId, sheets: Vec<Rc<Constructed>>) {
    let texts = sheets.iter().map(|c| (c.id, c.applied_text())).collect();
    if sheets.is_empty() {
        page.adopted_sheets.borrow_mut().remove(&scope);
    } else {
        page.adopted_sheets.borrow_mut().insert(scope, sheets);
    }
    page.set_adopted_sheets(scope, texts);
}

/// The sheets of an array-like `list`, each checked.
fn adoptable_list(s: &Scope<'_>, page: &PageDom, scope: NodeId, list: Object<'_>) -> JsResult<Vec<Rc<Constructed>>> {
    let len = to_u32(list.get("length")?)?;
    (0..len).map(|i| adoptable(s, page, scope, list.get_index(i)?)).collect()
}

/// `adoptedStyleSheets` of the document or shadow root `n`: one array per
/// scope and realm, made on first read.
pub(crate) fn adopted_style_sheets<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let holder = crate::node::wrap_node(s, n.page(), n.id)?;
    if let Some(array) = s.get_hidden(holder, ADOPTED)? {
        return Ok(array);
    }
    let target = s.array(&[])?;
    let sheets = n.page().adopted_sheets.borrow().get(&n.id).cloned().unwrap_or_default();
    for (i, c) in sheets.iter().enumerate() {
        target.set_index(i as u32, constructed_wrapper(s, c)?.value())?;
    }
    let navigator = crate::window::navigator(s, n.page())?
        .as_object()
        .ok_or_else(|| JsError::Type("adoptedStyleSheets is unavailable".into()))?;
    let factory = match s.get_hidden(navigator, ADOPTED_FACTORY)? {
        Some(f) => f,
        None => {
            let f = s.eval(ADOPTED_JS, Some("lightningpanda://internal/adopted-style-sheets.js"), 1)?;
            s.set_hidden(navigator, ADOPTED_FACTORY, f)?;
            f
        }
    };
    let scope = n.id;
    let check = s.function("check", move |s, _this, a| {
        let page = crate::target::page_of(s)?;
        adoptable(s, &page, scope, a.get(0))?;
        Ok(s.undefined())
    })?;
    let sync = s.function("sync", move |s, _this, a| {
        let page = crate::target::page_of(s)?;
        let list = a.get(0).as_object().ok_or_else(|| JsError::Type("not an array".into()))?;
        let sheets = adoptable_list(s, &page, scope, list)?;
        adopt(&page, scope, sheets);
        Ok(s.undefined())
    })?;
    let factory = factory.as_object().ok_or_else(|| JsError::Type("adoptedStyleSheets is unavailable".into()))?;
    let array = factory.call(None, &[target.value(), check.value(), sync.value()])?;
    s.set_hidden(holder, ADOPTED, array)?;
    s.set_hidden(holder, ADOPTED_TARGET, target.value())?;
    Ok(array)
}

/// `adoptedStyleSheets = value`: the sheets of the sequence `value`.
pub(crate) fn set_adopted_style_sheets(s: &Scope<'_>, n: &NodeData, value: Value<'_>) -> JsResult<()> {
    let list = value.as_object().filter(|o| !o.is_function()).ok_or_else(|| {
        JsError::Type(
            "Failed to set the 'adoptedStyleSheets' property: The provided value cannot be converted to a sequence."
                .into(),
        )
    })?;
    let sheets = adoptable_list(s, n.page(), n.id, list)?;
    // The array a script holds shows the new sheets.
    let holder = crate::node::wrap_node(s, n.page(), n.id)?;
    if let Some(target) = s.get_hidden(holder, ADOPTED_TARGET)?.and_then(|t| t.as_object()) {
        let values: Vec<Value<'_>> = (0..sheets.len() as u32).map(|i| list.get_index(i)).collect::<JsResult<_>>()?;
        target.set_atom("length", s.number(0.0))?;
        for (i, v) in values.into_iter().enumerate() {
            target.set_index(i as u32, v)?;
        }
    }
    adopt(n.page(), n.id, sheets);
    Ok(())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CssStyleDeclaration>();
    engine.register::<CssStyleSheet>();
    engine.register::<MediaList>();
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
        assert!(UNPARSED.windows(2).all(|w| w[0] < w[1]), "UNPARSED is sorted");
    }
}
