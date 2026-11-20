//! `CSSStyleDeclaration` for inline styles (`element.style`) backed by the
//! `style` attribute, and `CSSStyleSheet` for `<style>`/`<link>`, whose rules
//! are parsed from the sheet text when a script first asks for them.
//!
//! Declaration blocks are Stylo's (`pyro_dom::stylo::declarations`), as the
//! cascade parses them: invalid values are dropped, shorthands expand into
//! their longhands (`margin` sets `margin-top`, ...), values serialize as
//! CSSOM says (`#f00` reads back `rgb(255, 0, 0)`), and a write serializes
//! the whole block back. Properties Chrome has that the engine's Stylo does
//! not parse ([`UNPARSED`]) are kept as written. Computed styles come from
//! pyro-dom's style engine via `getComputedStyle`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use pyro_dom::cssom::{normalize_selector, parse_rules, Namespaces, PaletteRule, ParsedRule, PropertyRule};
use pyro_dom::stylo::declarations::{self, DeclarationBlock};
use pyro_dom::NodeId;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

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
    UNPARSED.binary_search(&name).is_ok() && !pyro_dom::stylo::is_property(name)
}

/// A CSS property name `CSSStyleDeclaration` reads and writes: one Stylo
/// knows, a custom property, or an [`UNPARSED`] one.
fn css_property(name: &str) -> bool {
    pyro_dom::stylo::is_property(name) || unparsed(name)
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
    fn is_font_face(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if matches!(node.kind, RuleKind::Other { rule_type: 5, .. }))
    }

    fn font_block(&self, text: &str) -> Option<pyro_dom::font_face::DescriptorBlock> {
        let StyleSource::Rule { owner, node } = &self.source else { return None };
        if !matches!(node.kind, RuleKind::Other { rule_type: 5, .. }) { return None; }
        let base = match owner {
            SheetOwner::Node(hold, _) => {
                let dom = hold.page.dom.borrow();
                let base = dom.document_base_url(hold.page.document()).unwrap_or("about:blank");
                hold.page.layout().style().sheet_base_url(&dom, hold.id, base).unwrap_or_else(|| base.to_owned())
            }
            SheetOwner::Constructed(sheet) => sheet.import.as_ref().map(|import| import.href.clone()).or_else(|| {
                Some(sheet.page.upgrade()?.dom.borrow().document_data(sheet.document).ok()?.url.to_string())
            }).unwrap_or_else(|| "about:blank".into()),
        };
        Some(pyro_dom::font_face::DescriptorBlock::parse(text, &base))
    }

    fn names(&self) -> Vec<String> {
        let text = self.text();
        self.font_block(&text).map_or_else(|| Block::parse(&text).names(), |block| block.names())
    }

    fn css_text(&self) -> String {
        let text = self.text();
        self.font_block(&text).map_or_else(|| Block::parse(&text).css_text(), |block| block.css_text())
    }
    fn is_position_try(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if matches!(&node.kind, RuleKind::Other { rule_type: 0, text } if text.borrow().starts_with("@position-try ")))
    }

    fn allows(&self, name: &str) -> bool {
        if self.is_font_face() { return pyro_dom::font_face::DescriptorBlock::supports(name); }
        !self.is_position_try() || DeclarationBlock::position_try_allows(name)
    }
    /// The block's text: the `style` attribute, or the rule's block.
    fn text(&self) -> String {
        match &self.source {
            StyleSource::Inline(el) => {
                el.page.dom.borrow().get_attribute(el.id, "style").ok().flatten().unwrap_or("").to_owned()
            }
            // pyro: (through the sheet object's accessor: nothing of a sheet not origin-clean.)
            StyleSource::Rule { owner, .. } if owner.access().is_err() => String::new(),
            StyleSource::Rule { node, .. } => match &node.kind {
                RuleKind::Style { block, .. } | RuleKind::Declarations(block) => block.borrow().clone(),
                // `@page` (its declarations, not its margin rules), margin and descriptor rules.
                RuleKind::Other { text, .. } => {
                    let text = text.borrow();
                    let Some((_, body)) = split_block(&text) else { return String::new() };
                    if is_page(node) { page_parts(body).0 } else { body.trim().to_owned() }
                }
                _ => String::new(),
            },
        }
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
                owner.access()?;
                match &node.kind {
                    RuleKind::Style { block, .. } | RuleKind::Declarations(block) => *block.borrow_mut() = text,
                    RuleKind::Other { .. } if is_page(node) => write_page(&*owner.rules()?, node, Some(&text)),
                    RuleKind::Other { text: rule, .. } => {
                        let head = split_block(&rule.borrow()).map(|(h, _)| h.to_owned());
                        let Some(head) = head else { return Ok(()) };
                        *rule.borrow_mut() =
                            if text.trim().is_empty() { format!("{head} {{ }}") } else { format!("{head} {{ {} }}", text.trim()) };
                        // A margin rule lives in its `@page` rule's text.
                        if let Some(page) = node.parent.upgrade().filter(|p| is_page(p)) {
                            write_page(&*owner.rules()?, &page, None);
                        }
                    }
                    RuleKind::Group { .. } | RuleKind::Property(_) | RuleKind::Palette(_) => return Ok(()),
                }
                owner.commit(&*owner.rules()?);
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
        if let Some(block) = self.font_block(&text) { return block.get(name); }
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
        if self.is_font_face() { return false; }
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
        if !self.allows(name) || (important && self.is_position_try()) {
            return Ok(());
        }
        let value = value.trim();
        if self.is_font_face() {
            let text = self.text();
            if let Some(mut block) = self.font_block(&text) {
                if (!important || value.is_empty()) && block.update(name, value) { self.write_raw(block.css_text())?; }
            }
            return Ok(());
        }
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
        if let Some(mut block) = self.font_block(&text) {
            let old = block.get(name);
            if block.update(name, "") { self.write_raw(block.css_text())?; }
            return Ok(old);
        }
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
        if let Some(block) = self.font_block(text) { return self.write_raw(block.css_text()); }
        if self.is_position_try() {
            return self.write_raw(DeclarationBlock::position_try(text).css_text());
        }
        self.write(&Block::parse(text))
    }
}

/// `CSSStyleProperties`: the declarations of a style attribute or a style
/// rule (a `CSSStyleDeclaration` with `cssFloat` and the property attributes).
pub struct CssStyleProperties;
impl JsClass for CssStyleProperties {
    const NAME: &'static str = "CSSStyleProperties";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "cssFloat",
            |s, st| Ok(s.string(&st.get("float"))),
            |_s, st, v| st.set("float", &dom_string(v)?, false),
        );
    }
}

pub struct CssStyleDeclaration;
impl JsClass for CssStyleDeclaration {
    const NAME: &'static str = "CSSStyleDeclaration";
    type Data = StyleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.indexed_iterator();
        def.accessor(
            "cssText",
            |s, st| Ok(s.string(&st.css_text())),
            |_s, st, v| st.set_css_text(&dom_string(v)?),
        );
        def.getter("length", |s, st| Ok(s.number(st.names().len() as f64)));
        def.getter("parentRule", |s, st| match &st.source {
            StyleSource::Inline(_) => Ok(s.null()),
            StyleSource::Rule { owner, node } => Ok(rule_wrapper(s, node, owner)?.value()),
        });
        def.pure_method("item", 1, |s, st, a| {
            a.require(1, "CSSStyleDeclaration.item")?;
            let i = to_u32(a.get(0))? as usize;
            Ok(s.string(st.names().get(i).map(String::as_str).unwrap_or("")))
        });
        def.indexed(|s, st, i| Ok(st.names().get(i as usize).map(|name| s.string(name))));
        def.pure_method("getPropertyValue", 1, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.getPropertyValue")?);
            Ok(s.string(&st.get(&name)))
        });
        def.pure_method("getPropertyPriority", 1, |s, st, a| {
            let name = property_arg(arg_string(&a, 0, "CSSStyleDeclaration.getPropertyPriority")?);
            Ok(s.string(if st.important(&name) { "important" } else { "" }))
        });
        def.method("setProperty", 2, |s, st, a| {
            a.require(2, "CSSStyleDeclaration.setProperty")?;
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
            if !st.allows(&property) { return Ok(None); }
            Ok(Some(s.string(&st.get(&property))))
        });
        def.named_setter(|_s, st, name, v| {
            let Some(property) = named_property(name) else { return Ok(false) };
            if !st.allows(&property) { return Ok(false); }
            let value = if v.is_null() { String::new() } else { dom_string(v)? };
            st.set(&property, &value, false)?;
            Ok(true)
        });
        def.property_names(|_s, st| Ok(st.names()));
    }
}

pub fn inline_style<'s>(s: &Scope<'s>, page: &PageDom, id: NodeId) -> JsResult<Object<'s>> {
    let base = page.node_identity(id);
    let key = IdentityKey(base.0 | (1 << 61) | (22 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    let page = crate::target::page_of(s)?;
    s.wrap::<CssStyleProperties>(StyleData { source: StyleSource::Inline(NodeHold::new(&page, id)?) }, Some(key))
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
    /// The margin rules of each `@page` rule of the sheet (by its identity),
    /// parsed from its text when that changes, so their objects stay the same.
    margins: RefCell<MarginCache>,
    /// pyro: the sheet object of its node the rules are of ([`SheetGeneration`]): another one (a
    /// repointed link's, a replaced `<style>`'s) gets rule objects of its own, even for the same
    /// text, never the previous object's rules (and their imports).
    generation: Cell<Option<SheetGeneration>>,
}

type MarginCache = std::collections::HashMap<(u64, u64), (Vec<String>, Vec<Rc<RuleNode>>)>;

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
    Declarations(RefCell<String>),
    Property(PropertyRule),
    Palette(PaletteRule),
    Style { selector: RefCell<String>, block: RefCell<String> },
    Group { name: String, condition: RefCell<String>, list: RuleList, bounds: Option<(Option<String>, Option<String>)> },
    Other { rule_type: u16, text: RefCell<String> },
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
                ParsedRule::Declarations(text) => RuleKind::Declarations(RefCell::new(text)),
                ParsedRule::Scope { start, end, rules } => {
                    let mut parts = Vec::new();
                    if let Some(start) = &start { parts.push(format!("({start})")); }
                    if let Some(end) = &end { parts.push(format!("to ({end})")); }
                    let list = RuleList::default();
                    list.replace(rules.into_iter().map(|r| RuleNode::new(r, me.clone())).collect());
                    RuleKind::Group { name: "scope".into(), condition: RefCell::new(parts.join(" ")), list, bounds: Some((start, end)) }
                }
                ParsedRule::Property(rule) => RuleKind::Property(rule),
                ParsedRule::Palette(rule) => RuleKind::Palette(rule),
                ParsedRule::Style { selector, block } => {
                    RuleKind::Style { selector: RefCell::new(selector), block: RefCell::new(block) }
                }
                ParsedRule::Group { name, condition, rules } => {
                    let list = RuleList::default();
                    list.replace(rules.into_iter().map(|r| RuleNode::new(r, me.clone())).collect());
                    RuleKind::Group { name, condition: RefCell::new(condition), list, bounds: None }
                }
                ParsedRule::Other { rule_type, text } => RuleKind::Other { rule_type, text: RefCell::new(text) },
            };
            RuleNode { identity, kind, parent }
        })
    }

    /// The text given to the style engine: blocks stay as written.
    fn source_text(&self) -> String {
        match &self.kind {
            RuleKind::Declarations(text) => text.borrow().clone(),
            RuleKind::Palette(rule) => rule.text.clone(),
            RuleKind::Property(rule) => rule.text.clone(),
            RuleKind::Style { selector, block } => format!("{} {{{}}}", selector.borrow(), block.borrow()),
            RuleKind::Group { name, condition, list, .. } => {
                let body = list.rules.borrow().iter().map(|r| r.source_text()).collect::<Vec<_>>().join("\n");
                format!("@{name} {} {{\n{body}\n}}", condition.borrow())
            }
            RuleKind::Other { text, .. } => text.borrow().clone(),
        }
    }

    fn css_text(&self) -> String {
        match &self.kind {
            RuleKind::Declarations(text) => DeclarationBlock::parse(&text.borrow()).css_text(),
            RuleKind::Palette(rule) => rule.text.clone(),
            RuleKind::Property(rule) => rule.text.clone(),
            RuleKind::Style { selector, block } => {
                // As the rule's `style.cssText` serializes the block.
                let decls = Block::parse(&block.borrow()).css_text();
                if decls.is_empty() {
                    format!("{} {{ }}", selector.borrow())
                } else {
                    format!("{} {{ {decls} }}", selector.borrow())
                }
            }
            RuleKind::Group { name, condition, list, .. } => {
                let prelude = condition.borrow();
                let mut out = if matches!(name.as_str(), "layer" | "scope") && prelude.is_empty() {
                    format!("@{name} {{\n")
                } else {
                    format!("@{name} {prelude} {{\n")
                };
                for rule in list.rules.borrow().iter() {
                    out.push_str("  ");
                    out.push_str(&rule.css_text());
                    out.push('\n');
                }
                out.push('}');
                out
            }
            RuleKind::Other { text, .. } => text.borrow().clone(),
        }
    }
}

fn sheet_source(page: &PageDom, owner: NodeId) -> String {
    let dom = page.dom.borrow();
    if dom.element(owner).is_ok_and(|e| *e.local_name() == *"style") {
        // Its child text content, as the style engine reads it.
        return dom.child_text_content(owner).into_owned();
    }
    // pyro: its own text, disabled or not (a disabled sheet keeps its rules).
    page.layout().style().sheet_own_text(owner).unwrap_or_default().to_owned()
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
            margins: RefCell::default(),
            generation: Cell::new(None),
        });
        page.sheet_models.borrow_mut().insert(owner, model.clone());
        model
    });
    model.checked.set(version);
    let generation = SheetGeneration::now(page, owner);
    let new_object = model.generation.replace(Some(generation)).is_some_and(|g| g != generation);
    if new_object || *model.source.borrow() != source {
        let mut namespaces = Vec::new();
        model.list.replace(parsed_nodes(&source, &mut namespaces));
        model.margins.borrow_mut().clear();
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
    // CSSOM "parse a CSS rule": one rule, and nothing after it but whitespace.
    let invalid = || JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'."));
    let in_scope = std::iter::successors(parent.upgrade(), |p| p.parent.upgrade()).any(|p| matches!(&p.kind, RuleKind::Group { bounds: Some(_), .. }));
    let mut parsed = vec![pyro_dom::cssom::parse_one_rule_in_scope(rule, &mut scoped, in_scope).map_err(|_| invalid())?];
    // CSSOM "insert a CSS rule": past the end is an IndexSizeError; @import and
    // @namespace only belong at the top of a sheet.
    if index > list.rules.borrow().len() {
        return Err(JsError::dom("IndexSizeError", "The index provided is larger than the number of rules."));
    }
    if parent.upgrade().is_some() && matches!(parsed[0], ParsedRule::Other { rule_type: IMPORT_RULE | 10, .. }) {
        return Err(JsError::dom("HierarchyRequestError", format!("'{rule}' may not be inserted inside a grouping rule.")));
    }
    // At the top of a sheet, @import (and @namespace) come before every other
    // rule but @layer statements: CSSOM "insert a CSS rule" step 5.
    if parent.upgrade().is_none() {
        let prologue = |kind: &RuleKind| match kind {
            RuleKind::Other { rule_type: IMPORT_RULE | 10, .. } => true,
            RuleKind::Other { rule_type: 0, text } => {
                let t = text.borrow();
                t.starts_with("@layer") && t.trim_end().ends_with(';')
            }
            _ => false,
        };
        let new_is_import = matches!(parsed[0], ParsedRule::Other { rule_type: IMPORT_RULE | 10, .. });
        let new_is_statement = matches!(&parsed[0], ParsedRule::Other { rule_type: 0, text } if text.starts_with("@layer") && text.trim_end().ends_with(';'));
        let rules = list.rules.borrow();
        let before_ok = rules[..index].iter().all(|r| prologue(&r.kind));
        let import_after = rules[index..].iter().any(|r| matches!(r.kind, RuleKind::Other { rule_type: IMPORT_RULE | 10, .. }));
        if (new_is_import && !before_ok) || (!new_is_import && !new_is_statement && import_after) {
            return Err(JsError::dom("HierarchyRequestError", format!("'{rule}' may not be inserted at index {index}.")));
        }
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

/// `cssRules` of a sheet (`group: None`) or of a grouping rule: a live
/// `CSSRuleList`, one object per holder and realm.
fn rule_list<'s>(s: &Scope<'s>, holder: IdentityKey, owner: &SheetOwner, group: Option<Rc<RuleNode>>) -> JsResult<Value<'s>> {
    let key = IdentityKey(holder.0 | (1 << 54), holder.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<CssRuleList>(RuleListData { owner: owner.hold()?, group }, Some(key))?.value())
}

pub struct RuleListData {
    owner: SheetOwner,
    group: Option<Rc<RuleNode>>,
}

impl RuleListData {
    /// pyro: through the sheet's one accessor (`SheetOwner::rules`), a grouping rule's too: a
    /// retained list of a sheet object that is detached, or not origin-clean, gives nothing.
    fn rules(&self) -> JsResult<Vec<Rc<RuleNode>>> {
        let model = self.owner.rules()?;
        Ok(match &self.group {
            Some(node) => match &node.kind {
                RuleKind::Group { list, .. } if is_attached(&model, node) => list.rules.borrow().clone(),
                _ if is_page(node) && is_attached(&model, node) => page_margins(&model, node),
                _ => Vec::new(),
            },
            None => model.list.rules.borrow().clone(),
        })
    }
}

/// `CSSRuleList`: live, indexed.
pub struct CssRuleList;
impl JsClass for CssRuleList {
    const NAME: &'static str = "CSSRuleList";
    type Data = RuleListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, l| Ok(s.number(l.rules()?.len() as f64)));
        def.pure_method("item", 1, |s, l, a| match l.rules()?.get({
            a.require(1, "CSSRuleList.item")?;
            to_u32(a.get(0))? as usize
        }) {
            Some(rule) => Ok(rule_wrapper(s, rule, &l.owner)?.value()),
            None => Ok(s.null()),
        });
        def.indexed(|s, l, i| match l.rules()?.get(i as usize) {
            Some(rule) => rule_wrapper(s, rule, &l.owner).map(|o| Some(o.value())),
            None => Ok(None),
        });
        def.indexed_iterator();
    }
}

/// The lowercase at-keyword of a rule kept as text (`import` of `@import ...`).
fn at_name(text: &str) -> String {
    text.trim_start_matches('@').split(|c: char| c.is_ascii_whitespace() || c == '{' || c == ';' || c == '(').next().unwrap_or("").to_ascii_lowercase()
}

fn rule_wrapper<'s>(s: &Scope<'s>, rule: &Rc<RuleNode>, owner: &SheetOwner) -> JsResult<Object<'s>> {
    if let Some(o) = s.lookup_identity(rule.identity) {
        return Ok(o);
    }
    let data = RuleData { owner: owner.hold()?, rule_node: rule.clone() };
    let key = Some(rule.identity);
    match &rule.kind {
        RuleKind::Declarations(_) => s.wrap::<CssNestedDeclarations>(data, key),
        RuleKind::Property(_) => s.wrap::<CssPropertyRule>(data, key),
        RuleKind::Palette(_) => s.wrap::<CssFontPaletteValuesRule>(data, key),
        RuleKind::Style { .. } => s.wrap::<CssStyleRule>(data, key),
        RuleKind::Group { name, .. } if name == "media" => s.wrap::<CssMediaRule>(data, key),
        RuleKind::Group { name, .. } if name == "supports" => s.wrap::<CssSupportsRule>(data, key),
        RuleKind::Group { name, .. } if name == "layer" => s.wrap::<CssLayerBlockRule>(data, key),
        RuleKind::Group { name, .. } if name == "container" => s.wrap::<CssContainerRule>(data, key),
        RuleKind::Group { bounds: Some(_), .. } => s.wrap::<CssScopeRule>(data, key),
        RuleKind::Group { .. } => s.wrap::<CssConditionRule>(data, key),
        RuleKind::Other { rule_type: COUNTER_STYLE_RULE, .. } => s.wrap::<CssCounterStyleRule>(data, key),
        RuleKind::Other { rule_type: MARGIN_RULE, .. } => s.wrap::<CssMarginRule>(data, key),
        RuleKind::Other { text, .. } => {
            let text = text.borrow();
            match at_name(&text).as_str() {
                "import" => s.wrap::<CssImportRule>(data, key),
                "font-face" => s.wrap::<CssFontFaceRule>(data, key),
                "page" => s.wrap::<CssPageRule>(data, key),
                "keyframes" | "-webkit-keyframes" => s.wrap::<CssKeyframesRule>(data, key),
                "namespace" => s.wrap::<CssNamespaceRule>(data, key),
                "font-feature-values" => s.wrap::<CssFontFeatureValuesRule>(data, key),
                "layer" if text.trim_end().ends_with(';') => s.wrap::<CssLayerStatementRule>(data, key),
                "layer" => s.wrap::<CssLayerBlockRule>(data, key),
                "starting-style" => s.wrap::<CssStartingStyleRule>(data, key),
                "position-try" => s.wrap::<CssPositionTryRule>(data, key),
                "view-transition" => s.wrap::<CssViewTransitionRule>(data, key),
                _ => s.wrap::<CssRule>(data, key),
            }
        }
    }
}

/// pyro: which sheet object of its node a sheet is: the node's style block generation and the
/// response generation (`PageDom::sheet_response`) it was made for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SheetGeneration {
    style: u32,
    response: u64,
}

impl SheetGeneration {
    fn now(page: &PageDom, owner: NodeId) -> Self {
        SheetGeneration {
            style: page.dom.borrow().style_sheet_generation(owner),
            response: page.sheet_response(owner).generation,
        }
    }
}

/// pyro: the parent identity of `owner`'s sheet object of `generation` for the sheets it imports
/// ([`pyro_dom::cssom::imported_sheet_key`]): a repointed link's (or a changed `<style>`'s) new
/// sheet object starts with no imports, never the previous one's.
pub(crate) fn sheet_parent_identity(owner: NodeId, generation: SheetGeneration) -> String {
    format!("{}#{}:{}", pyro_dom::cssom::sheet_owner_identity(owner), generation.style, generation.response)
}

/// pyro: [`sheet_parent_identity`] of `owner`'s sheet object now (the browser's, as it starts its
/// imports).
pub fn current_sheet_parent_identity(page: &PageDom, owner: NodeId) -> String {
    sheet_parent_identity(owner, SheetGeneration::now(page, owner))
}

fn sheet_key(page: &PageDom, owner: NodeId) -> IdentityKey {
    sheet_key_of(page, owner, SheetGeneration::now(page, owner))
}

/// The identity of `owner`'s sheet object of `generation`: a `<style>` whose children changed
/// has a new one (HTML "update a style block"), pyro: and a `<link>` that applied another
/// response.
fn sheet_key_of(page: &PageDom, owner: NodeId, generation: SheetGeneration) -> IdentityKey {
    sheet_object_key(page, owner, generation, 23)
}

/// pyro: the identity of an object of `owner`'s sheet object of `generation` (`kind`: the sheet
/// itself, 23; its media list, 25).
fn sheet_object_key(page: &PageDom, owner: NodeId, generation: SheetGeneration, kind: u64) -> IdentityKey {
    let base = page.node_identity(owner);
    let mixed = u64::from(generation.style).wrapping_add(generation.response.wrapping_mul(0x9E37_79B9));
    IdentityKey(base.0 | (1 << 61) | (kind << 48), base.1.wrapping_add(mixed << 24))
}

pub struct RuleData {
    owner: SheetOwner,
    /// pyro: reached only through [`RuleData::node`], which checks the sheet object it came from.
    rule_node: Rc<RuleNode>,
}

impl RuleData {
    /// pyro: the rule, through its sheet object's one accessor (`SheetOwner::access`): a rule of a
    /// sheet object whose response is not origin-clean gives a SecurityError; one of a detached
    /// object is its own, frozen content (never its node's newer rules).
    fn node(&self) -> JsResult<&Rc<RuleNode>> {
        self.owner.access()?;
        Ok(&self.rule_node)
    }
}

fn define_rule<C: JsClass<Data = RuleData>>(def: &mut ClassDef<C>) {
    for (name, value) in [
        ("STYLE_RULE", 1.0),
        ("CHARSET_RULE", 2.0),
        ("IMPORT_RULE", 3.0),
        ("MEDIA_RULE", 4.0),
        ("FONT_FACE_RULE", 5.0),
        ("PAGE_RULE", 6.0),
        ("KEYFRAMES_RULE", 7.0),
        ("KEYFRAME_RULE", 8.0),
        ("MARGIN_RULE", 9.0),
        ("NAMESPACE_RULE", 10.0),
        ("COUNTER_STYLE_RULE", 11.0),
        ("SUPPORTS_RULE", 12.0),
        ("FONT_FEATURE_VALUES_RULE", 14.0),
    ] {
        def.constant(name, value);
    }
    def.getter("type", |s, r| {
        Ok(s.number(match &r.node()?.kind {
            RuleKind::Style { .. } => 1.0,
            RuleKind::Group { name, .. } if name == "media" => 4.0,
            RuleKind::Group { name, .. } if name == "supports" => 12.0,
            RuleKind::Group { .. } | RuleKind::Property(_) | RuleKind::Palette(_) | RuleKind::Declarations(_) => 0.0,
            RuleKind::Other { rule_type, .. } => f64::from(*rule_type),
        }))
    });
    // Setting a rule's cssText does nothing (CSSOM).
    def.accessor("cssText", |s, r| Ok(s.string(&r.node()?.css_text())), |_s, _r, _v| Ok(()));
    def.getter("parentRule", |s, r| match r.node()?.parent.upgrade() {
        Some(parent) => Ok(rule_wrapper(s, &parent, &r.owner)?.value()),
        None => Ok(s.null()),
    });
    // Null once the rule (or the rule holding it) was removed from its sheet.
    def.getter("parentStyleSheet", |s, r| {
        let mut top = r.node()?.clone();
        while let Some(parent) = top.parent.upgrade() {
            top = parent;
        }
        let attached = r.owner.rules()?.list.rules.borrow().iter().any(|n| Rc::ptr_eq(n, &top));
        if attached { Ok(r.owner.wrapper(s)?.value()) } else { Ok(s.null()) }
    });
}

pub struct CssStyleRule;
impl JsClass for CssStyleRule {
    const NAME: &'static str = "CSSStyleRule";
    type Data = RuleData;
    // CSS Nesting: a style rule is a grouping rule (of nested rules).
    type Parent = CssGroupRule;
    fn define(def: &mut ClassDef<Self>) {
        // The rule's declaration block ([PutForwards=cssText]); one object
        // per rule and realm.
        def.accessor(
            "style",
            |s, r| {
                let key = IdentityKey(r.node()?.identity.0 | (1 << 55), r.node()?.identity.1);
                if let Some(o) = s.lookup_identity(key) {
                    return Ok(o.value());
                }
                let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
                Ok(s.wrap::<CssStyleProperties>(StyleData { source }, Some(key))?.value())
            },
            |_s, r, v| {
                let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
                StyleData { source }.set_css_text(&dom_string(v)?)
            },
        );
        // CSS Typed OM: the declaration block as a StylePropertyMap.
        def.getter("styleMap", |s, r| {
            let key = IdentityKey(r.node()?.identity.0 | (1 << 55), r.node()?.identity.1);
            let style = match s.lookup_identity(key) {
                Some(o) => o.value(),
                None => {
                    let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
                    s.wrap::<CssStyleProperties>(StyleData { source }, Some(key))?.value()
                }
            };
            let page = crate::target::page_of(s)?;
            crate::typed_om::declaration_map(s, &page, style)
        });
        def.accessor(
            "selectorText",
            |s, r| match &r.node()?.kind {
                RuleKind::Style { selector, .. } => Ok(s.string(&selector.borrow())),
                _ => Ok(s.undefined()),
            },
            |_s, r, v| {
                let text = dom_string(v)?;
                let model = r.owner.rules()?;
                let normalized = normalize_selector(&text, &model.namespaces.borrow());
                // An invalid selector leaves the rule unchanged.
                if let (Some(normalized), RuleKind::Style { selector, .. }) = (normalized, &r.node()?.kind) {
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
    type Parent = CssRule;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("cssRules", |s, r| {
            r.owner.rules()?;
            rule_list(s, r.node()?.identity, &r.owner, Some(r.node()?.clone()))
        });
        def.method("insertRule", 1, |s, r, a| {
            let rule = arg_string(&a, 0, "CSSGroupingRule.insertRule")?;
            let index = if a.get(1).is_undefined() { 0 } else { to_u32(a.get(1))? as usize };
            let RuleKind::Group { list, .. } = &r.node()?.kind else { return Ok(s.undefined()) };
            let model = r.owner.rules()?;
            let namespaces = model.namespaces.borrow().clone();
            insert_into(list, &namespaces, Rc::downgrade(r.node()?), &rule, index, r.owner.is_constructed())?;
            r.owner.commit(&model);
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, r, a| {
            a.require(1, "CSSGroupingRule.deleteRule")?;
            let index = to_u32(a.get(0))? as usize;
            let RuleKind::Group { list, .. } = &r.node()?.kind else { return Ok(s.undefined()) };
            delete_from(list, index)?;
            r.owner.commit(&*r.owner.rules()?);
            Ok(s.undefined())
        });
    }
}

/// `CSSRule`, the base of every rule interface (also the class of a rule
/// with no interface of its own).
pub struct CssRule;
impl JsClass for CssRule {
    const NAME: &'static str = "CSSRule";
    type Data = RuleData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        define_rule(def);
    }
}

/// `(head, body)` of an at-rule kept as text: before its `{` and inside it.
fn split_block(text: &str) -> Option<(&str, &str)> {
    let open = if text.starts_with("@position-try ") { text.find(" { ")? + 1 } else { text.find('{')? };
    let close = text.rfind('}').filter(|&c| c > open)?;
    Some((text[..open].trim_end(), &text[open + 1..close]))
}

/// A `@page` body split into its declarations and its margin at-rules.
fn page_parts(body: &str) -> (String, Vec<String>) {
    let (mut decls, mut margins) = (String::new(), Vec::new());
    let mut rest = body;
    while let Some(at) = rest.find('@') {
        decls.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(open) = after.find('{') else {
            rest = "";
            break;
        };
        let mut depth = 0;
        let mut end = after.len();
        for (i, c) in after[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        margins.push(after[..end].trim().to_owned());
        rest = &after[end..];
    }
    decls.push_str(rest);
    (decls.trim().to_owned(), margins)
}

/// The margin names of `@page` (CSS Paged Media 3 §4.2).
const MARGIN_RULES: &[&str] = &[
    "top-left-corner", "top-left", "top-center", "top-right", "top-right-corner", "bottom-left-corner",
    "bottom-left", "bottom-center", "bottom-right", "bottom-right-corner", "left-top", "left-middle",
    "left-bottom", "right-top", "right-middle", "right-bottom",
];

/// `CSSRule.type` of a margin rule.
const MARGIN_RULE: u16 = 9;


fn is_page(node: &RuleNode) -> bool {
    matches!(&node.kind, RuleKind::Other { rule_type: 6, .. })
}

/// The margin rules of a `@page` rule.
fn page_margins(model: &SheetModel, page: &Rc<RuleNode>) -> Vec<Rc<RuleNode>> {
    let RuleKind::Other { text, rule_type: 6 } = &page.kind else { return Vec::new() };
    let margins = split_block(&text.borrow()).map(|(_, body)| page_parts(body).1).unwrap_or_default();
    let margins: Vec<String> = margins
        .into_iter()
        .filter(|m| MARGIN_RULES.contains(&at_name(m).as_str()))
        .collect();
    let key = (page.identity.0, page.identity.1);
    {
        let mut cache = model.margins.borrow_mut();
        if let Some((texts, nodes)) = cache.get(&key) {
            if *texts == margins {
                return nodes.clone();
            }
        }
        let nodes: Vec<Rc<RuleNode>> = margins
            .iter()
            .map(|m| {
                let text = match split_block(m) {
                    Some((head, body)) if body.trim().is_empty() => format!("{head} {{ }}"),
                    Some((head, body)) => format!("{head} {{ {} }}", body.trim()),
                    None => m.clone(),
                };
                RuleNode::new(ParsedRule::Other { rule_type: MARGIN_RULE, text }, Rc::downgrade(page))
            })
            .collect();
        // Keep the texts as the page holds them, so an unchanged page reuses the nodes.
        cache.insert(key, (margins, nodes.clone()));
        nodes
    }
}

/// Rewrites a `@page` rule's text from its declarations and margin rules.
fn write_page(model: &SheetModel, page: &RuleNode, decls: Option<&str>) {
    let RuleKind::Other { text, .. } = &page.kind else { return };
    let current = text.borrow().clone();
    let Some((head, body)) = split_block(&current) else { return };
    let (old_decls, _) = page_parts(body);
    let decls = decls.unwrap_or(&old_decls).trim().to_owned();
    let key = (page.identity.0, page.identity.1);
    let margins: Vec<String> = model
        .margins
        .borrow()
        .get(&key)
        .map(|(_, nodes)| {
            nodes.iter().map(|n| match &n.kind { RuleKind::Other { text, .. } => text.borrow().clone(), _ => String::new() }).collect()
        })
        .unwrap_or_else(|| page_parts(body).1);
    let mut inner: Vec<String> = Vec::new();
    if !decls.is_empty() {
        inner.push(decls);
    }
    inner.extend(margins.iter().cloned());
    let body = if inner.is_empty() { " ".to_owned() } else { format!(" {} ", inner.join(" ")) };
    *text.borrow_mut() = format!("{head} {{{body}}}");
    // The cached margins now match the new text.
    if let Some(entry) = model.margins.borrow_mut().get_mut(&key) {
        entry.0 = margins;
    }
}

/// The text of a rule kept as written (`@import ...;`), or "".
fn rule_text(r: &RuleData) -> String {
    // pyro: (nothing of a sheet that may not be read: `RuleData::node`.)
    let Ok(node) = r.node() else { return String::new() };
    match &node.kind {
        RuleKind::Other { text, .. } => text.borrow().clone(),
        _ => String::new(),
    }
}

/// The prelude of a rule kept as text: between the at-keyword and `{` or `;`.
fn rule_prelude(r: &RuleData) -> String {
    let text = rule_text(r);
    let body = text.trim_start_matches('@');
    let body = body.find(|c: char| c.is_ascii_whitespace() || c == '{' || c == ';' || c == '(').map_or("", |i| &body[i..]);
    let end = body.find(['{', ';']).unwrap_or(body.len());
    body[..end].trim().to_owned()
}

/// A rule interface with no members of its own.
macro_rules! rule_class {
    ($name:ident, $js:literal, $parent:ty) => {
        rule_class!($name, $js, $parent, |_def| {});
    };
    ($name:ident, $js:literal, $parent:ty, $body:expr) => {
        pub struct $name;
        impl JsClass for $name {
            const NAME: &'static str = $js;
            type Data = RuleData;
            type Parent = $parent;
            fn define(def: &mut ClassDef<Self>) {
                let f: fn(&mut ClassDef<Self>) = $body;
                f(def);
            }
        }
    };
}

rule_class!(CssConditionRule, "CSSConditionRule", CssGroupRule, |def| {
    def.getter("conditionText", |s, r| match &r.node()?.kind {
        RuleKind::Group { condition, .. } => Ok(s.string(&condition.borrow())),
        _ => Ok(s.string(&rule_prelude(r))),
    });
});

/// `media` of `@media` and `@import` rules ([PutForwards=mediaText]): one
/// `MediaList` per rule and realm.
fn define_rule_media<C: JsClass<Data = RuleData>>(def: &mut ClassDef<C>) {
    def.accessor(
        "media",
        |s, r| {
            let key = IdentityKey(r.node()?.identity.0 | (1 << 53), r.node()?.identity.1);
            if let Some(o) = s.lookup_identity(key) {
                return Ok(o.value());
            }
            let data = RuleData { owner: r.owner.hold()?, rule_node: r.node()?.clone() };
            Ok(s.wrap::<MediaList>(MediaOwner::Rule(data), Some(key))?.value())
        },
        |_s, r, v| {
            let data = RuleData { owner: r.owner.hold()?, rule_node: r.node()?.clone() };
            MediaList::set(&MediaOwner::Rule(data), &dom_string(v)?)
        },
    );
}
rule_class!(CssMediaRule, "CSSMediaRule", CssConditionRule, |def| define_rule_media(def));
rule_class!(CssSupportsRule, "CSSSupportsRule", CssConditionRule);
/// The conditions of an `@container` rule (style engine serialization).
fn container_conditions(r: &RuleData) -> Vec<(String, String)> {
    let Ok(node) = r.node() else { return Vec::new() };
    let RuleKind::Group { condition, .. } = &node.kind else { return Vec::new() };
    pyro_dom::cssom::container_conditions(&condition.borrow()).unwrap_or_default()
}

rule_class!(CssContainerRule, "CSSContainerRule", CssConditionRule, |def| {
    // The name and query of a single condition; "" with several.
    def.getter("containerName", |s, r| {
        let c = container_conditions(r);
        Ok(s.string(if c.len() == 1 { &c[0].0 } else { "" }))
    });
    def.getter("containerQuery", |s, r| {
        let c = container_conditions(r);
        Ok(s.string(if c.len() == 1 { &c[0].1 } else { "" }))
    });
    // FrozenArray<CSSContainerCondition>.
    def.getter("conditions", |s, r| {
        let mut items = Vec::new();
        for (name, query) in container_conditions(r) {
            let o = s.object();
            o.set_atom("name", s.string(&name))?;
            o.set_atom("query", s.string(&query))?;
            items.push(o.value());
        }
        let array = s.array(&items)?;
        let freeze = s.global().get_atom("Object")?.as_object().expect("Object").get_atom("freeze")?;
        freeze.as_object().expect("Object.freeze").call(None, &[array.value()])
    });
});
rule_class!(CssImportRule, "CSSImportRule", CssRule, |def| {
    define_rule_media(def);
    def.getter("href", |s, r| Ok(s.string(&import_href(r))));
    // `layer` (anonymous: ""), `layer(name)`, or no layer (null).
    def.getter("layerName", |s, r| match import_parts(&rule_prelude(r)).0 {
        Some(name) => Ok(s.string(&name)),
        None => Ok(s.null()),
    });
    // The imported sheet once it loaded (one object per rule and realm).
    def.getter("styleSheet", |s, r| {
        // pyro: under the sheet object that imports it (by identity, never the request text alone).
        let parent = match &r.owner {
            SheetOwner::Node(h, generation) => sheet_parent_identity(h.id, *generation),
            SheetOwner::Constructed(c) => match &c.import {
                Some(i) => i.key.clone(),
                None => return Ok(s.null()),
            },
        };
        // pyro: one object per rule of that sheet object (a rule of a repointed link's new sheet
        // may have the old one's rule identity: its import object is never the old one's).
        let mixed = {
            use std::hash::{Hash, Hasher};
            let mut h = rustc_hash::FxHasher::default();
            parent.hash(&mut h);
            h.finish()
        };
        let key = IdentityKey(r.node()?.identity.0 | (1 << 52), r.node()?.identity.1 ^ mixed);
        if let Some(o) = s.lookup_identity(key) {
            return Ok(o.value());
        }
        let Some((page, url)) = import_url(r) else { return Ok(s.null()) };
        // pyro: the sheet of this rule's request (URL and modifiers), at its URL after redirects.
        let import = pyro_dom::cssom::import_request(&rule_prelude(r)).unwrap_or_default();
        let request = pyro_dom::cssom::imported_sheet_key(&parent, &pyro_dom::cssom::import_key(&url, &import));
        let Some((final_url, text, opaque)) = page.imported_sheets.borrow().get(&request).cloned() else {
            return Ok(s.null());
        };
        let media = media_text(&pyro_dom::cssom::import_split(&rule_prelude(r)).1);
        let rule = RuleData { owner: r.owner.hold()?, rule_node: r.node()?.clone() };
        let sheet = Constructed::imported(&page, final_url, &text, media, rule, opaque, request);
        Ok(s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(sheet), flag: None }, Some(key))?.value())
    });
    // The condition of `supports(...)`, or null.
    def.getter("supportsText", |s, r| match import_parts(&rule_prelude(r)).1 {
        Some(text) => Ok(s.string(&text)),
        None => Ok(s.null()),
    });
});
rule_class!(CssFontFaceRule, "CSSFontFaceRule", CssRule, |def| {
    def.accessor("style", rule_style::<CssFontFaceDescriptors>, |_s, rule, value| {
        let source = StyleSource::Rule { owner: rule.owner.hold()?, node: rule.node()?.clone() };
        StyleData { source }.set_css_text(&dom_string(value)?)
    });
});

pub struct CssFontFaceDescriptors;
impl JsClass for CssFontFaceDescriptors {
    const NAME: &'static str = "CSSFontFaceDescriptors";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(def: &mut ClassDef<Self>) {
        macro_rules! attributes {
            ($(($idl:literal, $css:literal)),*) => {$(
                def.accessor($idl, |s, st| Ok(s.string(&st.get($css))), |_s, st, value| st.set($css, &null_to_empty(value)?, false));
                if $idl != $css {
                    def.accessor($css, |s, st| Ok(s.string(&st.get($css))), |_s, st, value| st.set($css, &null_to_empty(value)?, false));
                }
            )*};
        }
        attributes!(("src", "src"), ("fontFamily", "font-family"), ("fontStyle", "font-style"), ("fontWeight", "font-weight"),
            ("fontStretch", "font-stretch"), ("fontWidth", "font-width"), ("sizeAdjust", "size-adjust"), ("unicodeRange", "unicode-range"),
            ("fontFeatureSettings", "font-feature-settings"), ("fontVariationSettings", "font-variation-settings"), ("fontDisplay", "font-display"),
            ("fontLanguageOverride", "font-language-override"), ("ascentOverride", "ascent-override"), ("descentOverride", "descent-override"),
            ("lineGapOverride", "line-gap-override"));
    }
}
/// `style` of a rule whose block holds declarations: one object per rule
/// and realm, of class `C`.
fn rule_style<'s, C: JsClass<Data = StyleData>>(s: &Scope<'s>, r: &RuleData) -> JsResult<Value<'s>> {
    let key = IdentityKey(r.node()?.identity.0 | (1 << 55), r.node()?.identity.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
    Ok(s.wrap::<C>(StyleData { source }, Some(key))?.value())
}

/// `CSSPageDescriptors`: the declarations of a `@page` rule.
pub struct CssPageDescriptors;
impl JsClass for CssPageDescriptors {
    const NAME: &'static str = "CSSPageDescriptors";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(def: &mut ClassDef<Self>) {
        // The page descriptors, by their IDL and dashed names.
        macro_rules! descriptors {
            ($(($idl:literal, $css:literal)),* $(,)?) => {$(
                def.accessor($idl, |s, st| Ok(s.string(&st.get($css))), |_s, st, v| st.set($css, &dom_string(v)?, false));
                if $idl != $css {
                    def.accessor($css, |s, st| Ok(s.string(&st.get($css))), |_s, st, v| st.set($css, &dom_string(v)?, false));
                }
            )*};
        }
        descriptors!(
            ("margin", "margin"),
            ("marginTop", "margin-top"),
            ("marginRight", "margin-right"),
            ("marginBottom", "margin-bottom"),
            ("marginLeft", "margin-left"),
            ("size", "size"),
            ("pageOrientation", "page-orientation"),
        );
    }
}

rule_class!(CssMarginRule, "CSSMarginRule", CssRule, |def| {
    def.getter("name", |s, r| Ok(s.string(&at_name(&rule_text(r)))));
    def.accessor("style", rule_style::<CssStyleProperties>, |_s, r, v| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData { source }.set_css_text(&dom_string(v)?)
    });
});

/// A page selector list as `CSSPageRule.selectorText` serializes it
/// (`name:first`, pseudo-pages lowercased), or `None` when it is invalid.
fn page_selector(text: &str) -> Option<String> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim_matches(|c: char| c.is_ascii_whitespace());
        let (name, pseudos) = part.split_at(part.find(':').unwrap_or(part.len()));
        let ident = |n: &str| n.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii());
        if !ident(name) || (name.is_empty() && pseudos.is_empty() && text.contains(',')) {
            return None;
        }
        let mut serialized = name.to_owned();
        for pseudo in pseudos.split(':').skip(1) {
            let lower = pseudo.to_ascii_lowercase();
            if !matches!(lower.as_str(), "first" | "left" | "right" | "blank") {
                return None;
            }
            serialized.push(':');
            serialized.push_str(&lower);
        }
        out.push(serialized);
    }
    Some(out.join(", "))
}

rule_class!(CssPageRule, "CSSPageRule", CssGroupRule, |def| {
    def.accessor("style", rule_style::<CssPageDescriptors>, |_s, r, v| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData { source }.set_css_text(&dom_string(v)?)
    });
    def.accessor(
        "selectorText",
        |s, r| Ok(s.string(&rule_prelude(r))),
        |_s, r, v| {
            let Some(selector) = page_selector(&dom_string(v)?) else { return Ok(()) };
            if let RuleKind::Other { text, .. } = &r.node()?.kind {
                let block = {
                    let t = text.borrow();
                    t.find('{').map_or_else(|| "{ }".to_owned(), |i| t[i..].to_owned())
                };
                let prefix = if selector.is_empty() { String::new() } else { format!("{selector} ") };
                *text.borrow_mut() = format!("@page {prefix}{block}");
                r.owner.commit(&*r.owner.rules()?);
            }
            Ok(())
        },
    );
});
rule_class!(CssKeyframesRule, "CSSKeyframesRule", CssRule, |def| {
    def.getter("name", |s, r| {
        let text = rule_text(r);
        let mut parser = cssparser::Parser::new(&text);
        let name = match parser.next() {
            Ok(cssparser::Token::AtKeyword(_)) => parser.expect_ident_or_string().ok().map(|name| name.as_ref()),
            _ => None,
        };
        Ok(s.string(name.unwrap_or_default()))
    });
});
rule_class!(CssNamespaceRule, "CSSNamespaceRule", CssRule, |def| {
    // `@namespace [prefix] (url(...) | "...")`.
    fn parts(r: &RuleData) -> (String, String) {
        let prelude = rule_prelude(r);
        let mut p = cssparser::Parser::new(&prelude);
        let (mut prefix, mut uri) = (String::new(), String::new());
        while let Ok(token) = p.next().cloned() {
            match token {
                cssparser::Token::Ident(v) => prefix = v.to_string(),
                cssparser::Token::QuotedString(v) | cssparser::Token::UnquotedUrl(v) => uri = v.to_string(),
                cssparser::Token::Function(f) if f.eq_ignore_ascii_case("url") => {
                    uri = p.parse_nested_block(|n| n.expect_string().map(|v| v.to_string()).map_err(cssparser::ParseError::<()>::from)).unwrap_or_default()
                }
                _ => {}
            }
        }
        (prefix, uri)
    }
    def.getter("namespaceURI", |s, r| Ok(s.string(&parts(r).1)));
    def.getter("prefix", |s, r| Ok(s.string(&parts(r).0)));
});
rule_class!(CssFontFeatureValuesRule, "CSSFontFeatureValuesRule", CssRule);
rule_class!(CssLayerBlockRule, "CSSLayerBlockRule", CssGroupRule, |def| {
    def.getter("name", |s, r| match &r.node()?.kind {
        RuleKind::Group { condition, .. } => Ok(s.string(&condition.borrow())),
        _ => Ok(s.string("")),
    });
});
rule_class!(CssLayerStatementRule, "CSSLayerStatementRule", CssRule, |def| {
    def.getter("nameList", |s, r| {
        let names: Vec<_> = rule_prelude(r).split(',').map(|n| n.trim().to_owned()).filter(|n| !n.is_empty()).collect();
        let values = names.iter().map(|n| s.string(n)).collect::<Vec<_>>();
        let array = s.array(&values)?;
        let freeze = s.global().get_atom("Object")?.as_object().expect("Object").get_atom("freeze")?;
        freeze.as_object().expect("Object.freeze").call(None, &[array.value()])
    });
});
fn property_rule(data: &RuleData) -> JsResult<&PropertyRule> {
    match &data.node()?.kind {
        RuleKind::Property(rule) => Ok(rule),
        _ => unreachable!(),
    }
}

rule_class!(CssNestedDeclarations, "CSSNestedDeclarations", CssRule, |def| {
    def.accessor("style", rule_style::<CssStyleProperties>, |_s, r, v| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData { source }.set_css_text(&dom_string(v)?)
    });
});

rule_class!(CssPropertyRule, "CSSPropertyRule", CssRule, |def| {
    def.getter("name", |s, r| Ok(s.string(&property_rule(r)?.name)));
    def.getter("syntax", |s, r| Ok(s.string(&property_rule(r)?.syntax)));
    def.getter("inherits", |s, r| Ok(s.boolean(property_rule(r)?.inherits)));
    def.getter("initialValue", |s, r| Ok(property_rule(r)?.initial_value.as_deref().map_or(s.null(), |v| s.string(v))));
});
rule_class!(CssFontPaletteValuesRule, "CSSFontPaletteValuesRule", CssRule, |def| {
    fn descriptors(data: &RuleData) -> JsResult<&PaletteRule> {
        match &data.node()?.kind { RuleKind::Palette(rule) => Ok(rule), _ => unreachable!() }
    }
    def.getter("name", |s, r| Ok(s.string(&descriptors(r)?.name)));
    def.getter("fontFamily", |s, r| Ok(s.string(&descriptors(r)?.font_family)));
    def.getter("basePalette", |s, r| Ok(s.string(&descriptors(r)?.base_palette)));
    def.getter("overrideColors", |s, r| Ok(s.string(&descriptors(r)?.override_colors)));
});
rule_class!(CssStartingStyleRule, "CSSStartingStyleRule", CssGroupRule);
rule_class!(CssScopeRule, "CSSScopeRule", CssGroupRule, |def| {
    def.getter("start", |s, r| match &r.node()?.kind {
        RuleKind::Group { bounds: Some((Some(start), _)), .. } => Ok(s.string(start)),
        _ => Ok(s.null()),
    });
    def.getter("end", |s, r| match &r.node()?.kind {
        RuleKind::Group { bounds: Some((_, Some(end))), .. } => Ok(s.string(end)),
        _ => Ok(s.null()),
    });
});
rule_class!(CssPositionTryRule, "CSSPositionTryRule", CssRule, |def| {
    def.getter("name", |s, r| {
        let text = rule_text(r);
        Ok(s.string(split_block(&text).and_then(|(head, _)| head.strip_prefix("@position-try ")).unwrap_or("")))
    });
    def.accessor("style", rule_style::<CssPositionTryDescriptors>, |_s, r, v| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData { source }.set_css_text(&dom_string(v)?)
    });
});

pub struct CssPositionTryDescriptors;
impl JsClass for CssPositionTryDescriptors {
    const NAME: &'static str = "CSSPositionTryDescriptors";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(_: &mut ClassDef<Self>) {}
}
rule_class!(CssViewTransitionRule, "CSSViewTransitionRule", CssRule);

/// `CSSRule.type` of `@import`.
const IMPORT_RULE: u16 = 3;

/// `CSSRule.type` of `@counter-style`.
const COUNTER_STYLE_RULE: u16 = 11;

/// The `(name, value)` declarations of a rule block, split at top-level `;` (quotes and brackets
/// kept whole).
fn block_declarations(block: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let (mut depth, mut quote, mut start) = (0i32, None::<char>, 0usize);
    let mut push = |piece: &str| {
        if let Some((name, value)) = piece.split_once(':') {
            out.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    };
    let mut escaped = false;
    for (i, c) in block.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match (quote, c) {
            (_, '\\') => escaped = true,
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(' | '[' | '{') => depth += 1,
            (None, ')' | ']' | '}') => depth -= 1,
            (None, ';') if depth == 0 => {
                push(&block[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&block[start..]);
    out
}

/// `CSSCounterStyleRule` (CSS Counter Styles 3 §6): `name` and the descriptors read and write
/// the rule's text; a change commits the sheet, so markers and counters follow it (WPT css-lists
/// `list-marker-counter-style-mutation`).
pub struct CssCounterStyleRule;
impl JsClass for CssCounterStyleRule {
    const NAME: &'static str = "CSSCounterStyleRule";
    type Data = RuleData;
    type Parent = CssRule;
    fn define(def: &mut ClassDef<Self>) {
        // (prelude name, block) of `@counter-style <name> {<block>}`.
        fn parts(text: &str) -> Option<(String, String)> {
            let open = text.find('{')?;
            let close = text.rfind('}')?;
            let name = text["@counter-style".len().min(open)..open].trim().to_string();
            Some((name, text[open + 1..close].to_string()))
        }
        fn text_of(r: &RuleData) -> Option<std::cell::Ref<'_, String>> {
            match &r.node().ok()?.kind {
                RuleKind::Other { text, .. } => Some(text.borrow()),
                _ => None,
            }
        }
        fn write(r: &RuleData, name: &str, declarations: &[(String, String)]) {
            let Ok(node) = r.node() else { return };
            if let RuleKind::Other { text, .. } = &node.kind {
                let body: String = declarations.iter().map(|(n, v)| format!(" {n}: {v};")).collect();
                *text.borrow_mut() = format!("@counter-style {name} {{{body} }}");
                // (Not for a detached or not origin-clean sheet: `SheetOwner::rules`.)
                if let Ok(model) = r.owner.rules() {
                    r.owner.commit(&model);
                }
            }
        }
        def.accessor(
            "name",
            |s, r| Ok(s.string(&text_of(r).and_then(|t| parts(&t)).map(|p| p.0).unwrap_or_default())),
            |_s, r, v| {
                let name = dom_string(v)?;
                let name = name.trim();
                let Some((_, block)) = text_of(r).and_then(|t| parts(&t)) else { return Ok(()) };
                // A name that is no identifier, or a CSS-wide keyword or `none`, is ignored.
                let valid = !name.is_empty()
                    && name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_' || !c.is_ascii())
                    && !name.starts_with(|c: char| c.is_ascii_digit())
                    && !matches!(name.to_ascii_lowercase().as_str(), "none" | "inherit" | "initial" | "unset" | "revert" | "revert-layer" | "default" | "decimal" | "disc");
                if valid {
                    write(r, name, &block_declarations(&block));
                }
                Ok(())
            },
        );
        fn get_descriptor(r: &RuleData, descriptor: &str) -> String {
            text_of(r)
                .and_then(|t| parts(&t))
                .and_then(|(_, block)| block_declarations(&block).into_iter().rev().find(|(n, _)| n == descriptor))
                .map(|(_, v)| v)
                .unwrap_or_default()
        }
        fn set_descriptor(r: &RuleData, descriptor: &str, value: &str) {
            let value = value.trim();
            // `system` cannot change after creation (CSSOM for counter styles); an empty value
            // is invalid.
            if value.is_empty() || descriptor == "system" {
                return;
            }
            let Some((name, block)) = text_of(r).and_then(|t| parts(&t)) else { return };
            let mut declarations: Vec<(String, String)> =
                block_declarations(&block).into_iter().filter(|(n, _)| n != descriptor).collect();
            declarations.push((descriptor.to_string(), value.to_string()));
            write(r, &name, &declarations);
        }
        macro_rules! descriptor {
            ($attr:literal, $descriptor:literal) => {
                def.accessor(
                    $attr,
                    |s, r| Ok(s.string(&get_descriptor(r, $descriptor))),
                    |_s, r, v| {
                        set_descriptor(r, $descriptor, &dom_string(v)?);
                        Ok(())
                    },
                );
            };
        }
        descriptor!("system", "system");
        descriptor!("symbols", "symbols");
        descriptor!("additiveSymbols", "additive-symbols");
        descriptor!("negative", "negative");
        descriptor!("prefix", "prefix");
        descriptor!("suffix", "suffix");
        descriptor!("range", "range");
        descriptor!("pad", "pad");
        descriptor!("speakAs", "speak-as");
        descriptor!("fallback", "fallback");
    }
}

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
    /// The sheet of an `@import` rule (`CSSImportRule.styleSheet`), not a
    /// constructed one: its URL and the rule that imports it.
    import: Option<ImportInfo>,
}

pub struct ImportInfo {
    href: String,
    rule: RuleData,
    /// pyro: the sheet's response was CORS-cross-origin: not origin-clean (CSSOM).
    opaque: bool,
    /// pyro: its key under the sheet that imports it (`cssom::imported_sheet_key`): its own
    /// imports' parent identity.
    key: String,
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
                margins: RefCell::default(),
                generation: Cell::new(None),
            }),
            media: RefCell::new(media_text(&media)),
            disabled: Cell::new(disabled),
            import: None,
        })
    }

    /// The sheet `rule` imports from `href`, with the rules of `text`. It
    /// applies nothing of its own: the style engine loads imports itself.
    fn imported(
        page: &Rc<PageDom>,
        href: String,
        text: &str,
        media: String,
        rule: RuleData,
        opaque: bool,
        key: String,
    ) -> Rc<Constructed> {
        static NEXT_SHEET: AtomicU64 = AtomicU64::new(1 << 40);
        let id = NEXT_SHEET.fetch_add(1, Ordering::Relaxed);
        let mut namespaces = Vec::new();
        let rules = parse_rules(text, &mut namespaces).into_iter().map(|r| RuleNode::new(r, Weak::new())).collect();
        let sheet = Rc::new(Constructed {
            id,
            page: Rc::downgrade(page),
            document: page.document(),
            identity: IdentityKey(rule.rule_node.identity.0 | (1 << 52), rule.rule_node.identity.1),
            model: Rc::new(SheetModel {
                source: RefCell::new(text.to_owned()),
                edited: Cell::new(false),
                list: RuleList::default(),
                namespaces: RefCell::new(namespaces),
                checked: Cell::new(0),
                margins: RefCell::default(),
                generation: Cell::new(None),
            }),
            media: RefCell::new(media),
            disabled: Cell::new(false),
            import: Some(ImportInfo { href, rule, opaque, key }),
        });
        sheet.model.list.replace(rules);
        sheet
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
        if self.import.is_some() {
            return;
        }
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
        self.model.margins.borrow_mut().clear();
        *self.model.namespaces.borrow_mut() = namespaces;
        self.commit();
    }
}

/// Whose sheet a CSSOM object belongs to.
pub enum SheetOwner {
    /// A `<style>`'s or `<link>`'s, for the response it was made for (`PageDom::sheet_response`:
    /// a link's next response is another sheet object).
    Node(NodeHold, SheetGeneration),
    Constructed(Rc<Constructed>),
}

impl SheetOwner {
    /// Another reference to the same sheet.
    fn hold(&self) -> JsResult<SheetOwner> {
        Ok(match self {
            SheetOwner::Node(h, generation) => SheetOwner::Node(NodeHold::new(&h.page, h.id)?, *generation),
            SheetOwner::Constructed(c) => SheetOwner::Constructed(c.clone()),
        })
    }
    /// pyro: the ONE way to a sheet's rules (CSSOM's rule lists, retained ones included, item,
    /// index and length, insertRule/deleteRule, an import rule's sheet, rule edits): a sheet
    /// object of a response that is no longer its node's (a link loaded another) is detached and
    /// has no rules; one whose response is not origin-clean throws a SecurityError.
    fn rules(&self) -> JsResult<Rc<SheetModel>> {
        match self {
            SheetOwner::Node(h, generation) => {
                let response = h.page.sheet_response(h.id);
                // (A `<style>` whose text was replaced, or a link that applied another response.)
                if *generation != SheetGeneration::now(&h.page, h.id) {
                    return Ok(detached_model());
                }
                if !response.origin_clean {
                    return Err(not_origin_clean());
                }
                Ok(sheet_model(&h.page, h.id))
            }
            SheetOwner::Constructed(c) if c.import.as_ref().is_some_and(|i| i.opaque) => Err(not_origin_clean()),
            SheetOwner::Constructed(c) => Ok(c.model.clone()),
        }
    }
    /// pyro: whether this sheet object may be read at all: a current object whose response is not
    /// origin-clean may not (a detached one holds only its own old content).
    fn access(&self) -> JsResult<()> {
        match self {
            SheetOwner::Node(h, generation) => {
                let current = *generation == SheetGeneration::now(&h.page, h.id);
                if current && !h.page.sheet_response(h.id).origin_clean {
                    return Err(not_origin_clean());
                }
                Ok(())
            }
            SheetOwner::Constructed(c) if c.import.as_ref().is_some_and(|i| i.opaque) => Err(not_origin_clean()),
            SheetOwner::Constructed(_) => Ok(()),
        }
    }
    /// pyro: whether this is its node's current sheet object.
    fn is_current(&self) -> bool {
        match self {
            SheetOwner::Node(h, generation) => *generation == SheetGeneration::now(&h.page, h.id),
            SheetOwner::Constructed(_) => true,
        }
    }
    /// A CSSOM edit of `model`'s rules (got from [`Self::rules`]); a detached sheet's go nowhere.
    fn commit(&self, model: &SheetModel) {
        match self {
            SheetOwner::Node(h, _) => {
                if self.is_current() {
                    commit_edit(&h.page, h.id, model)
                }
            }
            SheetOwner::Constructed(c) => c.commit(),
        }
    }
    fn key(&self) -> IdentityKey {
        match self {
            SheetOwner::Node(h, generation) => sheet_key_of(&h.page, h.id, *generation),
            SheetOwner::Constructed(c) => c.identity,
        }
    }
    fn is_constructed(&self) -> bool {
        matches!(self, SheetOwner::Constructed(c) if c.import.is_none())
    }
    fn wrapper<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        match self {
            SheetOwner::Node(h, _) => sheet_wrapper(s, &h.page, h.id),
            SheetOwner::Constructed(c) => constructed_wrapper(s, c),
        }
    }
}

/// pyro: whether `node` is (still) one of `model`'s rules, at any depth: a grouping rule kept
/// from a sheet whose text has been replaced since lists none of the new text's rules.
fn is_attached(model: &SheetModel, node: &Rc<RuleNode>) -> bool {
    fn within(rules: &[Rc<RuleNode>], node: &Rc<RuleNode>) -> bool {
        rules.iter().any(|r| {
            Rc::ptr_eq(r, node)
                || matches!(&r.kind, RuleKind::Group { list, .. } if within(&list.rules.borrow(), node))
        })
    }
    within(&model.list.rules.borrow(), node)
}

/// pyro: CSSOM: the rules of a sheet that is not origin-clean are not to be read or changed.
fn not_origin_clean() -> JsError {
    JsError::dom("SecurityError", "Cannot access rules")
}

/// pyro: the rules of a detached sheet object: none.
fn detached_model() -> Rc<SheetModel> {
    Rc::new(SheetModel {
        source: RefCell::new(String::new()),
        edited: Cell::new(false),
        list: RuleList::default(),
        namespaces: RefCell::default(),
        checked: Cell::new(0),
        margins: RefCell::default(),
        generation: Cell::new(None),
    })
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
    pyro_dom::cssom::serialize_media_list(text)
}

/// `CSSStyleSheet.media`: the `media` attribute of a sheet's owner, or a
/// constructed sheet's list.
pub struct MediaList;
/// Whose media list a `MediaList` is: a sheet's (its owner's `media`), an
/// `@media` rule's condition or an `@import` rule's media queries.
pub enum MediaOwner {
    Sheet(SheetOwner),
    Rule(RuleData),
}

/// The URL an `@import` rule names, as written.
fn import_href(r: &RuleData) -> String {
    // pyro: a `url()` with request URL modifiers too.
    pyro_dom::cssom::import_request(&rule_prelude(r)).map(|import| import.url).unwrap_or_default()
}

/// The page and absolute URL of the sheet an `@import` rule loads: its href
/// resolved against the importing sheet's location.
fn import_url(r: &RuleData) -> Option<(Rc<PageDom>, String)> {
    let href = import_href(r);
    match &r.owner {
        // pyro: a detached sheet object's import is of a response no longer there: no sheet.
        SheetOwner::Node(..) if !r.owner.is_current() => None,
        SheetOwner::Node(h, _) => {
            // pyro: the sheet's own base (`StyleEngine::sheet_base_url`) is its imports' base.
            let own = {
                let dom = h.page.dom.borrow();
                let document_base = dom.document_base_url(h.page.document()).ok().map(str::to_owned).unwrap_or_default();
                h.page.layout().style().sheet_base_url(&dom, h.id, &document_base)
            };
            let url = match own.and_then(|base| url::Url::parse(&base).ok()?.join(&href).ok()) {
                Some(url) => url.to_string(),
                None => crate::html::resolve_url(&h.page.dom.borrow(), h.id, &href),
            };
            Some((h.page.clone(), url))
        }
        SheetOwner::Constructed(c) => {
            let page = c.page.upgrade()?;
            let base = match &c.import {
                Some(i) => i.href.clone(),
                None => page.dom.borrow().document_data(c.document).ok()?.url.to_string(),
            };
            Some((page, url::Url::parse(&base).ok()?.join(&href).ok()?.to_string()))
        }
    }
}

/// The layer name (`Some("")` for an anonymous `layer`) and `supports()`
/// condition of an `@import` prelude, read from the tokens after its URL
/// (a `layer(...)` inside `supports()` or the media queries does not count).
fn import_parts(prelude: &str) -> (Option<String>, Option<String>) {
    let (head, _) = pyro_dom::cssom::import_split(prelude);
    let mut p = cssparser::Parser::new(&head);
    let (mut layer, mut supports) = (None, None);
    let _ = p.next();
    while let Ok(token) = p.next().cloned() {
        match token {
            cssparser::Token::Ident(i) if i.eq_ignore_ascii_case("layer") => layer = Some(String::new()),
            cssparser::Token::Function(f) => {
                let start = p.position();
                let _: Result<(), cssparser::ParseError<()>> = p.parse_nested_block(|n| {
                    while n.next().is_ok() {}
                    Ok(())
                });
                let inner = p.slice_from(start);
                let inner = inner.strip_suffix(')').unwrap_or(inner).trim().to_owned();
                if f.eq_ignore_ascii_case("layer") {
                    layer = Some(inner.split('.').map(str::trim).collect::<Vec<_>>().join("."));
                } else if f.eq_ignore_ascii_case("supports") {
                    supports = Some(inner);
                }
            }
            _ => {}
        }
    }
    (layer, supports)
}

impl MediaList {
    fn text(m: &MediaOwner) -> String {
        let sh = match m {
            MediaOwner::Sheet(sh) => sh,
            MediaOwner::Rule(r) => {
                let Ok(node) = r.node() else { return String::new() };
                return match &node.kind {
                    RuleKind::Group { condition, .. } => media_text(&condition.borrow()),
                    _ => media_text(&pyro_dom::cssom::import_split(&rule_prelude(r)).1),
                }
            }
        };
        match sh {
            // pyro: a detached sheet object's list is no longer its node's.
            SheetOwner::Node(..) if !sh.is_current() => String::new(),
            SheetOwner::Node(h, _) => {
                media_text(h.page.dom.borrow().get_attribute(h.id, "media").ok().flatten().unwrap_or(""))
            }
            SheetOwner::Constructed(c) => c.media.borrow().clone(),
        }
    }
    fn set(m: &MediaOwner, text: &str) -> JsResult<()> {
        let text = media_text(text);
        let sh = match m {
            MediaOwner::Sheet(sh) => sh,
            MediaOwner::Rule(r) => {
                match &r.node()?.kind {
                    RuleKind::Group { condition, .. } => *condition.borrow_mut() = text,
                    RuleKind::Other { text: rule, .. } => {
                        let (head, _) = pyro_dom::cssom::import_split(&rule_prelude(r));
                        let media = if text.is_empty() { String::new() } else { format!(" {text}") };
                        *rule.borrow_mut() = format!("@import {head}{media};");
                    }
                    RuleKind::Style { .. } | RuleKind::Property(_) | RuleKind::Palette(_) | RuleKind::Declarations(_) => return Ok(()),
                }
                r.owner.commit(&*r.owner.rules()?);
                return Ok(());
            }
        };
        match sh {
            // pyro: a detached sheet object's list writes nowhere (never the replacement's media).
            SheetOwner::Node(..) if !sh.is_current() => Ok(()),
            SheetOwner::Node(h, _) => h.page.dom.borrow_mut().set_attribute(h.id, "media", &text).js(),
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
    type Data = MediaOwner;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.accessor(
            "mediaText",
            |s, sh| Ok(s.string(&MediaList::text(sh))),
            |_s, sh, v| MediaList::set(sh, &null_to_empty(v)?),
        );
        def.getter("length", |s, sh| Ok(s.number(media_queries(&MediaList::text(sh)).len() as f64)));
        def.pure_method("item", 1, |s, sh, a| {
            a.require(1, "MediaList.item")?;
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
        def.pure_method("toString", 0, |s, sh, _a| Ok(s.string(&MediaList::text(sh))));
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
    s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(sheet), flag: None }, Some(key))
}

fn constructed_wrapper<'s>(s: &Scope<'s>, c: &Rc<Constructed>) -> JsResult<Object<'s>> {
    if let Some(o) = s.lookup_identity(c.identity) {
        return Ok(o);
    }
    s.wrap::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(c.clone()), flag: None }, Some(c.identity))
}

pub struct SheetData {
    owner: SheetOwner,
    /// pyro: a node's sheet object's `disabled` flag, shared by its wrappers in every realm and,
    /// while it is current, by its node's association (`sheet_flag`).
    flag: Option<Rc<Cell<bool>>>,
}

/// pyro: the `disabled` cell of `node`'s sheet object of `generation`: the association's while it
/// is the current one (made on first use, replacing a previous generation's, whose cell only its
/// wrappers keep); a fresh unset one for a detached generation nothing holds. The map keeps one
/// entry per live node.
pub(crate) fn sheet_flag(page: &PageDom, node: NodeId, generation: SheetGeneration) -> Rc<Cell<bool>> {
    let mut flags = page.sheet_flags.borrow_mut();
    if let Some((g, flag)) = flags.get(&node) {
        if *g == generation {
            return flag.clone();
        }
    }
    let flag = Rc::new(Cell::new(false));
    if generation == SheetGeneration::now(page, node) {
        if let Ok(dom) = page.dom.try_borrow() {
            flags.retain(|n, _| dom.is_alive(*n));
        }
        flags.insert(node, (generation, flag.clone()));
    }
    flag
}

/// pyro: how many sheet associations hold a `disabled` cell (one per live node at most).
pub fn sheet_flag_count(page: &PageDom) -> usize {
    page.sheet_flags.borrow().len()
}

/// pyro: applies `disabled` of `node`'s current sheet object in the style engine (that style block
/// generation's flag: an updated `<style>` block is enabled at once).
fn apply_sheet_disabled(page: &PageDom, node: NodeId, disabled: bool) {
    let dom = page.dom.borrow();
    page.layout.borrow_mut().style_mut().set_sheet_disabled(&dom, node, disabled);
}

/// pyro: `disabled` of `node`'s current sheet object (`HTMLStyleElement.disabled`): the same cell
/// as its wrappers' `disabled`.
pub(crate) fn current_sheet_disabled(page: &PageDom, node: NodeId) -> bool {
    sheet_flag(page, node, SheetGeneration::now(page, node)).get()
}

/// pyro: sets `disabled` of `node`'s current sheet object (`HTMLStyleElement.disabled`).
pub(crate) fn set_current_sheet_disabled(page: &PageDom, node: NodeId, disabled: bool) {
    sheet_flag(page, node, SheetGeneration::now(page, node)).set(disabled);
    apply_sheet_disabled(page, node, disabled);
}

/// pyro: a link applied another response (a new sheet object, enabled): the style engine's flag,
/// which was the previous object's, no longer applies. Called as the association changes.
pub(crate) fn new_link_sheet(page: &PageDom, node: NodeId) {
    let dom = page.dom.borrow();
    page.layout.borrow_mut().style_mut().set_sheet_disabled(&dom, node, false);
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

/// `StyleSheet`, the base of `CSSStyleSheet`: type, href, owner node, title,
/// media and disabled.
pub struct StyleSheet;
impl JsClass for StyleSheet {
    const NAME: &'static str = "StyleSheet";
    type Data = SheetData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("ownerNode", |s, sh| match &sh.owner {
            SheetOwner::Node(h, _) => Ok(crate::node::wrap_node(s, &h.page, h.id)?.value()),
            SheetOwner::Constructed(_) => Ok(s.null()),
        });
        def.getter("parentStyleSheet", |s, sh| match &sh.owner {
            SheetOwner::Constructed(c) => match &c.import {
                Some(i) => Ok(i.rule.owner.wrapper(s)?.value()),
                None => Ok(s.null()),
            },
            SheetOwner::Node(..) => Ok(s.null()),
        });
        def.getter("title", |s, sh| match &sh.owner {
            SheetOwner::Node(h, _) => {
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
                    // pyro: one media list per sheet object (its generation in the key, as the
                    // sheet's own): a replaced sheet's list is not the new sheet's.
                    SheetOwner::Node(h, generation) => sheet_object_key(&h.page, h.id, *generation, 25),
                    SheetOwner::Constructed(c) => IdentityKey((1 << 56) | (1 << 53), c.id),
                };
                if let Some(o) = s.lookup_identity(key) {
                    return Ok(o.value());
                }
                Ok(s.wrap::<MediaList>(MediaOwner::Sheet(sh.owner.hold()?), Some(key))?.value())
            },
            |_s, sh, v| MediaList::set(&MediaOwner::Sheet(sh.owner.hold()?), &dom_string(v)?),
        );
        def.getter("href", |s, sh| match &sh.owner {
            SheetOwner::Node(h, _) => {
                let dom = h.page.dom.borrow();
                Ok(match dom.get_attribute(h.id, "href").js()? {
                    Some(href) => s.string(&crate::html::resolve_url(&dom, h.id, href)),
                    None => s.null(),
                })
            }
            // A constructed sheet's location is its document's URL.
            SheetOwner::Constructed(c) if c.import.is_some() => Ok(s.string(&c.import.as_ref().map(|i| i.href.clone()).unwrap_or_default())),
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
            |s, sh| {
                Ok(s.boolean(match &sh.owner {
                    SheetOwner::Constructed(c) => c.disabled.get(),
                    // pyro: the sheet object's own flag (`sheet_flag`), current or detached.
                    SheetOwner::Node(..) => sh.flag.as_ref().is_some_and(|f| f.get()),
                }))
            },
            |_s, sh, v| {
                match &sh.owner {
                    SheetOwner::Constructed(c) => {
                        if c.disabled.replace(v.to_bool()) != v.to_bool() {
                            c.push();
                        }
                    }
                    // A `<style>`'s or `<link>`'s sheet: it applies nothing while disabled.
                    // pyro: this sheet object's flag; only the current one's applies.
                    // pyro: only the current sheet object's applies; a detached one's toggles nothing.
                    SheetOwner::Node(h, generation) => {
                        if let Some(flag) = &sh.flag {
                            flag.set(v.to_bool());
                        }
                        if *generation == SheetGeneration::now(&h.page, h.id) {
                            apply_sheet_disabled(&h.page, h.id, v.to_bool());
                        }
                    }
                }
                Ok(())
            },
        );
    }
}

pub struct CssStyleSheet;
impl JsClass for CssStyleSheet {
    const NAME: &'static str = "CSSStyleSheet";
    type Data = SheetData;
    type Parent = StyleSheet;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let (media, disabled) = sheet_options(a.get(0))?;
            let page = crate::target::page_of(s)?;
            let sheet = Constructed::new(&page, media, disabled);
            let key = sheet.identity;
            s.wrap_new::<CssStyleSheet>(SheetData { owner: SheetOwner::Constructed(sheet), flag: None }, nt, Some(key))
        });
        def.getter("ownerRule", |s, sh| match &sh.owner {
            SheetOwner::Constructed(c) => match &c.import {
                Some(i) => Ok(rule_wrapper(s, i.rule.node()?, &i.rule.owner)?.value()),
                None => Ok(s.null()),
            },
            SheetOwner::Node(..) => Ok(s.null()),
        });
        def.getter("cssRules", rules_value);
        def.getter("rules", rules_value);
        def.method("insertRule", 1, |s, sh, a| {
            let rule = arg_string(&a, 0, "CSSStyleSheet.insertRule")?;
                    let index = if a.get(1).is_undefined() { 0 } else { to_u32(a.get(1))? as usize };
            let model = sh.owner.rules()?;
            let namespaces = model.namespaces.borrow().clone();
            insert_into(&model.list, &namespaces, Weak::new(), &rule, index, sh.owner.is_constructed())?;
            sh.owner.commit(&model);
            Ok(s.number(index as f64))
        });
        def.method("addRule", 0, |s, sh, a| {
            // Legacy: `addRule(selector, style, index)` inserts
            // `selector { style }` at `index` (the end by default); -1.
            let selector = if a.get(0).is_undefined() { "undefined".to_owned() } else { dom_string(a.get(0))? };
                    let block = if a.get(1).is_undefined() { "undefined".to_owned() } else { dom_string(a.get(1))? };
            let model = sh.owner.rules()?;
            let index =
                if a.get(2).is_undefined() { model.list.rules.borrow().len() } else { to_u32(a.get(2))? as usize };
            let namespaces = model.namespaces.borrow().clone();
            let rule = format!("{selector} {{ {block} }}");
            insert_into(&model.list, &namespaces, Weak::new(), &rule, index, sh.owner.is_constructed())?;
            sh.owner.commit(&model);
            Ok(s.number(-1.0))
        });
        def.method("deleteRule", 1, |s, sh, a| {
            a.require(1, "CSSStyleSheet.deleteRule")?;
            let index = to_u32(a.get(0))? as usize;
                    let model = sh.owner.rules()?;
            delete_from(&model.list, index)?;
            sh.owner.commit(&model);
            Ok(s.undefined())
        });
        def.method("removeRule", 0, |s, sh, a| {
            let index = if a.get(0).is_undefined() { 0 } else { to_u32(a.get(0))? as usize };
                    let model = sh.owner.rules()?;
            delete_from(&model.list, index)?;
            sh.owner.commit(&model);
            Ok(s.undefined())
        });
        def.method("replaceSync", 1, |s, sh, a| {
            let text = arg_string(&a, 0, "CSSStyleSheet.replaceSync")?;
            let Some(c) = (match &sh.owner {
                SheetOwner::Constructed(c) if c.import.is_none() => Some(c),
                _ => None,
            }) else {
                return Err(not_constructed("replaceSync"));
            };
            c.replace(&text);
            Ok(s.undefined())
        });
        def.promise_method("replace", 1, |s, sh, a| {
            let deferred = s.deferred()?;
            let promise = deferred.promise.get(s);
            // A promise-returning operation: argument errors reject.
            let text = match arg_string(&a, 0, "CSSStyleSheet.replace") {
                Ok(text) => text,
                Err(e) => {
                    deferred.reject(s, e.to_value(s))?;
                    return Ok(promise);
                }
            };
            match &sh.owner {
                SheetOwner::Constructed(c) if c.import.is_none() => {
                    c.replace(&text);
                    deferred.resolve(s, constructed_wrapper(s, c)?.value())?;
                }
                _ => deferred.reject(s, not_constructed("replace").to_value(s))?,
            }
            Ok(promise)
        });
    }
}

fn rules_value<'s>(s: &Scope<'s>, sh: &SheetData) -> JsResult<Value<'s>> {
    // pyro: the getter throws too, not only the list's members (`SheetOwner::rules`).
    sh.owner.rules()?;
    rule_list(s, sh.owner.key(), &sh.owner, None)
}

/// pyro: the data of `owner`'s current sheet object's wrapper, with its `disabled` cell.
fn node_sheet_data(page: &Rc<PageDom>, owner: NodeId) -> JsResult<SheetData> {
    let generation = SheetGeneration::now(page, owner);
    Ok(SheetData { owner: SheetOwner::Node(NodeHold::new(page, owner)?, generation), flag: Some(sheet_flag(page, owner, generation)) })
}

fn sheet_wrapper<'s>(s: &Scope<'s>, page: &Rc<PageDom>, owner: NodeId) -> JsResult<Object<'s>> {
    let key = sheet_key(page, owner);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o);
    }
    s.wrap::<CssStyleSheet>(node_sheet_data(page, owner)?, Some(key))
}

pub(crate) fn element_sheet<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let connected = n.page().dom.borrow().is_connected(n.id).js()?;
    if !connected {
        return Ok(s.null());
    }
    // pyro: a link with the `disabled` attribute has no sheet, at once (Chrome); enabled again, it
    // gets a new one (the loader loads it again: a new response, a new sheet object).
    let disabled_link = {
        let dom = n.page().dom.borrow();
        dom.element(n.id).is_ok_and(|e| e.is_html() && &*e.name.local == "link" && e.attr("disabled").is_some())
    };
    if disabled_link {
        return Ok(s.null());
    }
    sheet_object(s, n.page(), n.id)
}

/// The CSSStyleSheet of the connected `<style>` or `<link>` element `owner`.
fn sheet_object<'s>(s: &Scope<'s>, page: &Rc<PageDom>, owner: NodeId) -> JsResult<Value<'s>> {
    let key = sheet_key(page, owner);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    Ok(s.wrap::<CssStyleSheet>(node_sheet_data(page, owner)?, Some(key))?.value())
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
                            // pyro: a disabled link has no sheet (Chrome).
                            && e.attr("disabled").is_none()
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
        def.pure_method("item", 1, |s, root, a| match StyleSheetList::owners(root).get({
            a.require(1, "StyleSheetList.item")?;
            to_u32(a.get(0))? as usize
        }) {
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
    // pyro: an `@import`ed sheet (`CSSImportRule.styleSheet`) is not a constructed one either.
    let c = match &sheet.owner {
        SheetOwner::Constructed(c) if c.import.is_none() => c,
        _ => return Err(JsError::dom("NotAllowedError", "Can't adopt non-constructed stylesheets.")),
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
            let f = s.eval(ADOPTED_JS, Some("pyro://internal/adopted-style-sheets.js"), 1)?;
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

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<CssStyleDeclaration>();
    engine.register::<CssPositionTryDescriptors>();
    engine.register::<CssStyleProperties>();
    engine.register::<StyleSheet>();
    engine.register::<CssStyleSheet>();
    engine.register::<MediaList>();
    engine.register::<StyleSheetList>();
    engine.register::<CssRuleList>();
    engine.register::<CssRule>();
    engine.register::<CssStyleRule>();
    engine.register::<CssGroupRule>();
    engine.register::<CssConditionRule>();
    engine.register::<CssMediaRule>();
    engine.register::<CssSupportsRule>();
    engine.register::<CssContainerRule>();
    engine.register::<CssImportRule>();
    engine.register::<CssFontFaceRule>();
    engine.register::<CssFontFaceDescriptors>();
    engine.register::<CssPageRule>();
    engine.register::<CssPageDescriptors>();
    engine.register::<CssMarginRule>();
    engine.register::<CssKeyframesRule>();
    engine.register::<CssNamespaceRule>();
    engine.register::<CssFontFeatureValuesRule>();
    engine.register::<CssLayerBlockRule>();
    engine.register::<CssLayerStatementRule>();
    engine.register::<CssPropertyRule>();
    engine.register::<CssNestedDeclarations>();
    engine.register::<CssFontPaletteValuesRule>();
    engine.register::<CssStartingStyleRule>();
    engine.register::<CssScopeRule>();
    engine.register::<CssPositionTryRule>();
    engine.register::<CssViewTransitionRule>();
    engine.register::<CssCounterStyleRule>();
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
