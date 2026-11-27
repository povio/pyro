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

use pyro_dom::cssom::{normalize_relative_selector, parse_rules, CounterDescriptor, CounterRule, FeatureRule, Namespaces, PaletteRule, ParsedRule, ParseRelative, PropertyRule};
use pyro_dom::stylo::declarations::{self, CssRuleType, DeclarationBlock, TypedStyleValues};
use pyro_dom::NodeId;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::collections::NodeHold;
use crate::node::NodeData;
use crate::page::PageDom;
use crate::util::*;

pub use pyro_dom::style::Declaration;

pub fn parse_declarations(text: &str) -> Vec<Declaration> {
    let mut declarations: Vec<Declaration> = Vec::new();
    for mut declaration in pyro_dom::style::parse_declarations(text) {
        if !declaration.name.starts_with("--") {
            declaration.name.make_ascii_lowercase();
        }
        declarations.retain(|previous| previous.name != declaration.name);
        declarations.push(declaration);
    }
    declarations
}

pub fn serialize_declarations(decls: &[Declaration]) -> String {
    decls
        .iter()
        .map(|d| {
            let mut name = String::with_capacity(d.name.len());
            cssparser::serialize_identifier(&d.name, &mut name).unwrap();
            format!("{name}: {}{};", d.value, if d.important { " !important" } else { "" })
        })
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
    if name.is_empty() {
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
    // Owned by a live CSSOM wrapper, never by ordinary DOM elements.
    edits: RefCell<Option<Box<DeclarationEdits>>>,
}

struct DeclarationEdits {
    canonical: String,
    semantic: Option<String>,
    slots: Vec<(String, std::ops::Range<usize>, Option<std::ops::Range<usize>>)>,
}

impl DeclarationEdits {
    fn source(&self) -> &str { self.semantic.as_deref().unwrap_or(&self.canonical) }

    fn independent_range(text: &str, name: &str) -> Option<std::ops::Range<usize>> {
        use cssparser::{Delimiter, ParseError, Parser};
        if !text.contains(name) { return None }
        let mut parser = Parser::new(text);
        let mut names = Vec::new();
        let mut found = None;
        while !parser.is_exhausted() {
            parser.skip_whitespace();
            let start = parser.position().byte_index();
            let prop = parser.expect_ident_cloned().ok()?;
            parser.expect_colon().ok()?;
            parser.parse_until_before(Delimiter::Semicolon, |input| -> Result<(), ParseError<()>> {
                while input.next().is_ok() {}
                Ok(())
            }).ok()?;
            parser.expect_semicolon().ok()?;
            if prop.as_ref() == name {
                if found.is_some() { return None }
                found = Some(start..parser.position().byte_index());
            }
            names.push(prop);
        }
        let found = found?;
        declarations::serializes_independently(name, names.iter().map(|p| p.as_ref())).then_some(found)
    }

    fn slot(&mut self, name: &str) -> Option<usize> {
        if let Some(i) = self.slots.iter().position(|(n, ..)| n == name) { return Some(i) }
        let canonical = Self::independent_range(&self.canonical, name)?;
        let semantic = match &self.semantic {
            Some(text) => {
                let range = Self::independent_range(text, name)?;
                // An exposed zero may stand for a negative zero in this very longhand.
                if self.canonical[canonical.clone()] != text[range.clone()] { return None }
                Some(range)
            }
            None => None,
        };
        if self.slots.len() == 4 { self.slots.remove(0); }
        self.slots.push((name.to_owned(), canonical, semantic));
        Some(self.slots.len() - 1)
    }

    fn replace(&mut self, slot: usize, css: &str) {
        fn replace_ranges<'a>(
            text: &mut String, old: std::ops::Range<usize>, css: &str,
            ranges: impl Iterator<Item = &'a mut std::ops::Range<usize>>,
        ) {
            let delta = css.len() as isize - old.len() as isize;
            text.replace_range(old.clone(), css);
            for r in ranges {
                if r.start >= old.end {
                    r.start = r.start.checked_add_signed(delta).unwrap();
                    r.end = r.end.checked_add_signed(delta).unwrap();
                } else if r.start == old.start {
                    r.end = r.start + css.len();
                }
            }
        }
        let canonical = self.slots[slot].1.clone();
        let semantic = self.slots[slot].2.clone();
        replace_ranges(&mut self.canonical, canonical, css, self.slots.iter_mut().map(|s| &mut s.1));
        if let (Some(text), Some(range)) = (&mut self.semantic, semantic) {
            replace_ranges(text, range, css, self.slots.iter_mut().filter_map(|s| s.2.as_mut()));
        }
    }
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
        self.with_kept(self.parsed.css_text())
    }
    fn css_with_typed(&self, typed: Option<&TypedStyleValues>) -> String {
        match typed {
            Some(values) => self.with_kept(self.parsed.css_with_typed_values(values)),
            None => self.css_text(),
        }
    }
    fn with_kept(&self, parsed: String) -> String {
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
    fn new(source: StyleSource) -> Self { Self { source, edits: RefCell::default() } }

    fn is_keyframe(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if matches!(node.kind, RuleKind::Other { rule_type: 8, .. }))
    }

    fn is_page_rule(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if is_page(node))
    }

    fn is_font_face(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if matches!(node.kind, RuleKind::Other { rule_type: 5, .. }))
    }

    fn font_block(&self, text: &str) -> Option<pyro_dom::font_face::DescriptorBlock> {
        let StyleSource::Rule { owner, node } = &self.source else { return None };
        if !matches!(node.kind, RuleKind::Other { rule_type: 5, .. }) { return None; }
        Some(owner.font_descriptors(text))
    }

    fn names(&self) -> Vec<String> {
        let text = self.text();
        if let Some(rule) = self.declaration_rule() { return DeclarationBlock::parse_in_rule(&text, rule).names(); }
        self.font_block(&text).map_or_else(|| Block::parse(&text).names(), |block| block.names())
    }

    fn css_text(&self) -> String {
        let (text, _, typed) = self.text_and_backing();
        if let Some(rule) = self.declaration_rule() { return DeclarationBlock::parse_in_rule(&text, rule).css_text(); }
        self.font_block(&text).map_or_else(|| Block::parse(&text).css_with_typed(typed.as_ref()), |block| block.css_text())
    }
    fn is_position_try(&self) -> bool {
        matches!(&self.source, StyleSource::Rule { node, .. } if matches!(&node.kind, RuleKind::Other { rule_type: 0, text } if text.borrow().starts_with("@position-try ")))
    }

    fn declaration_rule(&self) -> Option<CssRuleType> {
        if self.is_keyframe() { return Some(CssRuleType::Keyframe); }
        if self.is_page_rule() { return Some(CssRuleType::Page); }
        if matches!(&self.source, StyleSource::Rule { node, .. } if matches!(node.kind, RuleKind::Other { rule_type: MARGIN_RULE, .. })) {
            return Some(CssRuleType::Margin);
        }
        self.is_position_try().then_some(CssRuleType::PositionTry)
    }

    fn allows(&self, name: &str) -> bool {
        if self.is_font_face() { return pyro_dom::font_face::DescriptorBlock::supports(name); }
        self.declaration_rule().is_none_or(|rule| DeclarationBlock::allowed_in_rule(name, rule))
    }
    /// The block's text: the `style` attribute, or the rule's block.
    fn text(&self) -> String { self.text_and_backing().0 }

    fn text_and_backing(&self) -> (String, Option<bool>, Option<TypedStyleValues>) {
        match &self.source {
            StyleSource::Inline(el) => {
                // pyro: a `style` attribute CSP blocked declares nothing (as Chrome: the element's
                // inline style is empty), from the moment it was set.
                crate::csp::resolve_inline_styles(&el.page);
                let dom = el.page.dom.borrow();
                if dom.style_attribute_blocked(el.id) {
                    return (String::new(), None, None);
                }
                let Ok(e) = dom.element(el.id) else { return (String::new(), None, None) };
                (e.semantic_inline_style().or_else(|| e.attr_local(&markup5ever::local_name!("style")))
                    .unwrap_or("").to_owned(), e.checked_inline_style_backing(), e.typed_style_values().cloned())
            }
            StyleSource::Rule { owner, .. } if owner.access().is_err() => (String::new(), None, None),
            StyleSource::Rule { node, .. } => match &node.kind {
                RuleKind::Style { block, .. } | RuleKind::Declarations(block) =>
                    (block.borrow().clone(), node.checked_style_backing.get(), node.typed_values.borrow().clone()),
                RuleKind::Other { text, .. } => {
                    let text = text.borrow();
                    let Some((_, body)) = split_block(&text) else { return (String::new(), None, None) };
                    (if is_page(node) { page_parts(body).0 } else { body.trim().to_owned() }, None, None)
                }
                _ => (String::new(), None, None),
            },
        }
    }

    /// An independently serialized longhand can be replaced in both strings directly.
    /// Keying by the actual source also checks edits through other wrappers and DOM APIs.
    fn replace_independent(&self, text: &str, signed: bool, name: &str, value: &str, important: bool) -> JsResult<bool> {
        let mut cache = self.edits.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.source() != text || c.semantic.is_some() != signed) {
            if !text.contains(name) { return Ok(false) }
            let canonical = if signed {
                match &self.source {
                    StyleSource::Inline(el) => el.page.dom.borrow().get_attribute(el.id, "style").ok().flatten().unwrap_or("").to_owned(),
                    StyleSource::Rule { .. } => Block::parse(text).css_text(),
                }
            } else { text.to_owned() };
            *cache = Some(Box::new(DeclarationEdits { canonical, semantic: signed.then(|| text.to_owned()), slots: Vec::new() }));
        }
        let edit = cache.as_mut().unwrap();
        let Some(slot) = edit.slot(name) else { return Ok(false) };
        let Some(css) = declarations::declaration_text(name, value, important) else { return Ok(true) };
        if !css.strip_prefix(name).is_some_and(|tail| tail.starts_with(':')) { return Ok(false) }
        if edit.canonical[edit.slots[slot].1.clone()] == css {
            // An unchanged public string may hide a typed-value change. Let the signed
            // path compare the parsed declaration as it does for uncached writes.
            return Ok(!signed);
        }
        edit.replace(slot, &css);
        let (canonical, semantic) = (edit.canonical.clone(), edit.semantic.clone());
        drop(cache);
        self.write_semantic(canonical, semantic)?;
        Ok(true)
    }

    /// Serializes `block` back into the attribute or the rule.
    fn write(&self, block: &Block) -> JsResult<()> {
        let canonical = block.css_text();
        let semantic = block.parsed.css_for_reparse().map(|css| block.with_kept(css)).filter(|css| *css != canonical);
        self.write_semantic(canonical, semantic)
    }
    /// Writes `text` unless it is what the block already holds (`before`):
    /// an unchanged block is no attribute mutation.
    fn write_text(&self, before: &str, text: String) -> JsResult<()> {
        if text == before {
            return Ok(());
        }
        self.write_raw(text)
    }
    /// Rule blocks are the renderer's input. CSSOM reads serialize that input canonically.
    fn write_semantic(&self, canonical: String, semantic: Option<String>) -> JsResult<()> {
        self.write_origins(canonical, semantic, None)
    }

    fn write_origins(&self, canonical: String, semantic: Option<String>, typed: Option<TypedStyleValues>) -> JsResult<()> {
        match &self.source {
            StyleSource::Inline(el) => el.page.dom.borrow_mut().set_cssom_inline_style_with_typed_values(el.id, canonical, semantic, typed).js(),
            StyleSource::Rule { node, .. } => match &node.kind {
                RuleKind::Style { .. } | RuleKind::Declarations(_) => {
                    let signed = semantic.is_some();
                    self.write_raw(semantic.unwrap_or(canonical))?;
                    node.checked_style_backing.set(Some(signed));
                    *node.typed_values.borrow_mut() = typed;
                    Ok(())
                }
                _ => self.write_raw(canonical),
            },
        }
    }
    fn write_raw(&self, text: String) -> JsResult<()> {
        match &self.source {
            StyleSource::Inline(el) => el.page.dom.borrow_mut().set_cssom_inline_style(el.id, text, None).js(),
            StyleSource::Rule { owner, node } => {
                owner.access()?;
                match &node.kind {
                    RuleKind::Style { block, .. } | RuleKind::Declarations(block) => {
                        *block.borrow_mut() = text;
                        node.checked_style_backing.set(Some(false));
                        node.typed_values.borrow_mut().take();
                    }
                    RuleKind::Other { .. } if is_page(node) => write_page(&*owner.rules()?, node, Some(&text)),
                    RuleKind::Other { text: rule, .. } => {
                        let head = split_block(&rule.borrow()).map(|(h, _)| h.to_owned());
                        let Some(head) = head else { return Ok(()) };
                        *rule.borrow_mut() =
                            if text.trim().is_empty() { format!("{head} {{ }}") } else { format!("{head} {{ {} }}", text.trim()) };
                        // A margin rule lives in its `@page` rule's text.
                        if let Some(page) = node.parent.borrow().upgrade().filter(|p| is_page(p)) {
                            write_page(&*owner.rules()?, &page, None);
                        }
                    }
                    RuleKind::Features(_) | RuleKind::Counter(_) | RuleKind::Group { .. } | RuleKind::Property(_) | RuleKind::Palette(_) | RuleKind::ViewTransition { .. } => return Ok(()),
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
        let (text, _, typed) = self.text_and_backing();
        if let Some((_, value)) = typed.as_ref().and_then(|v| v.iter().find(|(n, _)| n == name)) {
            return value.clone();
        }
        if let Some(rule) = self.declaration_rule() { return DeclarationBlock::parse_in_rule(&text, rule).value(name); }
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
        if let Some(rule) = self.declaration_rule() { return DeclarationBlock::parse_in_rule(&text, rule).important(name); }
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
    fn signed_block(text: &str, known: Option<bool>) -> Option<Block> {
        match known {
            Some(false) => None,
            Some(true) => Some(Block::parse(text)),
            None if declarations::may_need_signed_zero_backing(text) => {
                let block = Block::parse(text);
                block.parsed.css_for_reparse().is_some().then_some(block)
            }
            None => None,
        }
    }

    pub(crate) fn typed_text(&self, name: &str) -> JsResult<Option<String>> {
        let find = |typed: Option<&TypedStyleValues>| {
            typed.and_then(|values| values.iter().find(|(n, _)| n == name)).map(|(_, v)| v.clone())
        };
        Ok(match &self.source {
            StyleSource::Inline(el) => find(el.page.dom.borrow().element(el.id).js()?.typed_style_values()),
            StyleSource::Rule { owner, node } => { owner.access()?; find(node.typed_values.borrow().as_ref()) }
        })
    }

    /// Mutate parsed values for cascade and retain the specified serialization for
    /// typed longhands. CSS text writes revoke the origin for every affected longhand.
    fn edit_typed(
        &self, text: &str, name: &str, value: &str, important: bool,
        prior: Option<TypedStyleValues>, from_object: bool,
    ) -> JsResult<String> {
        let mut block = Block::parse(text);
        let mut typed = prior.as_ref().map_or_else(Vec::new, |v| v.as_ref().clone());
        let old = typed.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone()).unwrap_or_else(|| block.parsed.value(name));
        let changed = if unparsed(name) {
            let i = block.kept.iter().position(|d| d.name == name);
            if value.is_empty() {
                if let Some(i) = i { let old = block.kept.remove(i).value; self.publish_typed(&block, typed)?; return Ok(old) }
                return Ok(old);
            }
            let decl = Declaration { name: name.into(), value: value.into(), important };
            if let Some(i) = i { block.kept[i] = decl; } else { block.kept.push(decl); }
            true
        } else if value.is_empty() {
            block.parsed.remove(name)
        } else {
            let Some(changed) = block.parsed.set(name, value, important) else { return Ok(old) };
            changed
        };
        typed.retain(|(n, _)| (from_object && n == name) || !declarations::overlaps(n, name));
        if from_object {
            if let Some((_, old_value)) = typed.iter_mut().find(|(n, _)| n == name) {
                value.clone_into(old_value);
            } else {
                typed.push((name.to_owned(), value.to_owned()));
            }
        }
        if changed || typed.as_slice() != prior.as_ref().map_or(&[][..], |v| v.as_slice()) {
            self.publish_typed(&block, typed)?;
        }
        Ok(old)
    }

    fn publish_typed(&self, block: &Block, values: Vec<(String, String)>) -> JsResult<()> {
        let typed = (!values.is_empty()).then(|| std::sync::Arc::new(values));
        let canonical = block.css_with_typed(typed.as_ref());
        let semantic = block.parsed.css_for_reparse().map(|v| block.with_kept(v)).filter(|v| *v != canonical);
        self.write_origins(canonical, semantic, typed)
    }

    pub(crate) fn set_typed(&self, name: &str, value: &str) -> JsResult<()> {
        if !self.allows(name) || self.declaration_rule().is_some() { return Ok(()); }
        let (source, _, typed) = self.text_and_backing();
        self.edit_typed(&source, name, value, false, typed, true)?;
        Ok(())
    }

    /// `setProperty(name, value, priority)`: an empty value removes the
    /// property; an invalid one, or a name that is no property, changes
    /// nothing. When no other declaration sets a longhand of `name`, the new
    /// declaration replaces its own in place and the others stay as written;
    /// otherwise the whole block is parsed, updated and serialized.
    fn set(&self, name: &str, value: &str, important: bool) -> JsResult<()> {
        if !self.allows(name) || (important && (self.is_position_try() || self.is_keyframe())) {
            return Ok(());
        }
        let value = value.trim();
        if let Some(rule) = self.declaration_rule().filter(|_| !value.is_empty()) {
            let mut block = DeclarationBlock::parse_in_rule(&self.text(), rule);
            if block.set_in_rule(name, value, important, rule) == Some(true) { self.write_raw(block.source_text())?; }
            return Ok(());
        }
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
        let (text, known, typed) = self.text_and_backing();
        if typed.is_some() { self.edit_typed(&text, name, value, important, typed, false)?; return Ok(()) }
        let may_need_backing = declarations::may_need_signed_zero_backing(value);
        if let Some(signed) = known {
            if !may_need_backing && text.len() <= 8192 && !unparsed(name)
                && self.replace_independent(&text, signed, name, value, important)? {
                return Ok(());
            }
        }
        let mut incoming = None;
        let mut signed = Self::signed_block(&text, known);
        if signed.is_none() && !unparsed(name) && may_need_backing {
            let mut declaration = DeclarationBlock::parse("");
            if declaration.set(name, value, important).is_none() { return Ok(()); }
            if declaration.css_for_reparse().is_some() { signed = Some(Block::parse(&text)); }
            incoming = Some(declaration);
        }
        if let Some(mut block) = signed {
            if unparsed(name) {
                match block.kept.iter_mut().find(|d| d.name == name) {
                    Some(d) if d.value == value && d.important == important => return Ok(()),
                    Some(d) => { d.value = value.into(); d.important = important; }
                    None => block.kept.push(Declaration { name: name.into(), value: value.into(), important }),
                }
            } else if block.parsed.set(name, value, important) != Some(true) {
                return Ok(());
            }
            return self.write(&block);
        }
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
        let declaration = match incoming {
            Some(block) => block.css_text(),
            None => match declarations::declaration_text(name, value, important) {
                Some(css) => css,
                None => return Ok(()),
            },
        };
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
        let (text, known, typed) = self.text_and_backing();
        if typed.is_some() { return self.edit_typed(&text, name, "", false, typed, false) }
        if let Some(rule) = self.declaration_rule() {
            let mut block = DeclarationBlock::parse_in_rule(&text, rule);
            let old = block.value(name);
            if block.remove(name) { self.write_raw(block.source_text())?; }
            return Ok(old);
        }
        if let Some(mut block) = self.font_block(&text) {
            let old = block.get(name);
            if block.update(name, "") { self.write_raw(block.css_text())?; }
            return Ok(old);
        }
        if !text.contains(':') {
            return Ok(String::new());
        }
        if let Some(mut block) = Self::signed_block(&text, known) {
            let old = if unparsed(name) {
                let Some(i) = block.kept.iter().position(|d| d.name == name) else { return Ok(String::new()) };
                block.kept.remove(i).value
            } else {
                let old = block.parsed.value(name);
                if !block.parsed.remove(name) { return Ok(old); }
                old
            };
            self.write(&block)?;
            return Ok(old);
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
        if let Some(rule) = self.declaration_rule() {
            return self.write_raw(DeclarationBlock::parse_in_rule(text, rule).source_text());
        }
        if let Some(block) = self.font_block(text) { return self.write_raw(block.css_text()); }
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
            let prio = if a.get(2).is_undefined() { String::new() } else { null_to_empty(a.get(2))? };
            if value.is_empty() {
                st.remove(&name)?;
                return Ok(s.undefined());
            }
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
    s.wrap::<CssStyleProperties>(StyleData::new(StyleSource::Inline(NodeHold::new(&page, id)?)), Some(key))
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
    Counter(RefCell<CounterRule>),
    Features(RefCell<FeatureRule>),
    ViewTransition { navigation: String, types: Vec<String>, text: String },
    Declarations(RefCell<String>),
    Property(PropertyRule),
    Palette(PaletteRule),
    Style { selector: RefCell<String>, block: RefCell<String>, list: RuleList },
    Group { name: String, condition: RefCell<String>, list: RuleList, bounds: Option<(Option<String>, Option<String>)> },
    Other { rule_type: u16, text: RefCell<String> },
}

struct RuleNode {
    typed_values: RefCell<Option<TypedStyleValues>>,
    checked_style_backing: Cell<Option<bool>>,
    identity: IdentityKey,
    kind: RuleKind,
    parent: RefCell<Weak<RuleNode>>,
}

impl RuleNode {
    fn new(parsed: ParsedRule, parent: Weak<RuleNode>) -> Rc<RuleNode> {
        static NEXT_RULE: AtomicU64 = AtomicU64::new(1);
        let identity = IdentityKey(1 << 56, NEXT_RULE.fetch_add(1, Ordering::Relaxed));
        Rc::new_cyclic(|me| {
            let kind = match parsed {
                ParsedRule::ViewTransition { navigation, types, text } => RuleKind::ViewTransition { navigation, types, text },
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
                ParsedRule::Features(rule) => RuleKind::Features(RefCell::new(rule)),
                ParsedRule::Counter(rule) => RuleKind::Counter(RefCell::new(rule)),
                ParsedRule::Palette(rule) => RuleKind::Palette(rule),
                ParsedRule::Style { selector, block, rules } => {
                    let list = RuleList::default();
                    list.replace(rules.into_iter().map(|rule| RuleNode::new(rule, me.clone())).collect());
                    RuleKind::Style { selector: RefCell::new(selector), block: RefCell::new(block), list }
                }
                ParsedRule::Group { name, condition, rules } => {
                    let list = RuleList::default();
                    list.replace(rules.into_iter().map(|r| RuleNode::new(r, me.clone())).collect());
                    RuleKind::Group { name, condition: RefCell::new(condition), list, bounds: None }
                }
                ParsedRule::Other { rule_type, text } => RuleKind::Other { rule_type, text: RefCell::new(text) },
            };
            RuleNode { identity, kind, parent: RefCell::new(parent), checked_style_backing: Cell::new(None), typed_values: RefCell::default() }
        })
    }

    /// The text given to the style engine: blocks stay as written.
    fn source_text(&self) -> String {
        match &self.kind {
            RuleKind::ViewTransition { text, .. } => text.clone(),
            RuleKind::Declarations(text) => text.borrow().clone(),
            RuleKind::Counter(rule) => rule.borrow().text(),
            RuleKind::Features(rule) => rule.borrow().text(),
            RuleKind::Palette(rule) => rule.text.clone(),
            RuleKind::Property(rule) => rule.text.clone(),
            RuleKind::Style { selector, block, list } => {
                let children = list.rules.borrow().iter().map(|rule| rule.source_text()).collect::<Vec<_>>().join("\n");
                let block = block.borrow();
                let separator = if !children.is_empty() && !block.trim_end().ends_with(';') { ";" } else { "" };
                format!("{} {{{block}{separator}{children}}}", selector.borrow())
            },
            RuleKind::Group { name, condition, list, .. } => {
                let body = list.rules.borrow().iter().map(|r| r.source_text()).collect::<Vec<_>>().join("\n");
                format!("@{name} {} {{\n{body}\n}}", condition.borrow())
            }
            RuleKind::Other { text, .. } => text.borrow().clone(),
        }
    }

    fn css_text(&self, owner: &SheetOwner) -> String {
        match &self.kind {
            RuleKind::ViewTransition { text, .. } => text.clone(),
            RuleKind::Other { rule_type: 5, text } => {
                let text = text.borrow();
                let body = split_block(&text).map_or("", |(_, body)| body);
                let descriptors = owner.font_descriptors(body).css_text();
                if descriptors.is_empty() { "@font-face { }".into() } else { format!("@font-face {{ {descriptors} }}") }
            }
            RuleKind::Other { rule_type: 8, text } => {
                let text = text.borrow();
                let Some((head, body)) = split_block(&text) else { return String::new() };
                let decls = DeclarationBlock::keyframe(body).css_text();
                if decls.is_empty() { format!("{head} {{ }}") } else { format!("{head} {{ {decls} }}") }
            }
            RuleKind::Other { rule_type: MARGIN_RULE, text } => {
                let text = text.borrow();
                canonical_margin_text(&text).unwrap_or_else(|| text.clone())
            }
            RuleKind::Other { rule_type, text } if *rule_type == 6 || text.borrow().starts_with("@position-try ") => {
                let text = text.borrow();
                if !declarations::may_need_signed_zero_backing(&text) { return text.clone(); }
                let Some((head, body)) = split_block(&text) else { return text.clone() };
                let (block, mut margins) = if *rule_type == 6 {
                    let (decls, margins) = page_parts(body);
                    (DeclarationBlock::page(&decls), margins)
                } else {
                    (DeclarationBlock::position_try(body), Vec::new())
                };
                let mut changed = block.css_for_reparse().is_some();
                for margin in &mut margins {
                    if let Some(canonical) = canonical_margin_text(margin) {
                        *margin = canonical;
                        changed = true;
                    }
                }
                if !changed { return text.clone(); }
                let inner = std::iter::once(block.css_text()).chain(margins)
                    .filter(|part| !part.is_empty()).collect::<Vec<_>>().join(" ");
                format!("{head} {{ {inner} }}")
            }
            RuleKind::Declarations(text) => Block::parse(&text.borrow()).css_with_typed(self.typed_values.borrow().as_ref()),
            RuleKind::Counter(rule) => rule.borrow().text(),
            RuleKind::Features(rule) => rule.borrow().text(),
            RuleKind::Palette(rule) => rule.text.clone(),
            RuleKind::Property(rule) => rule.text.clone(),
            RuleKind::Style { selector, block, list } => {
                // As the rule's `style.cssText` serializes the block.
                let decls = Block::parse(&block.borrow()).css_with_typed(self.typed_values.borrow().as_ref());
                if !list.rules.borrow().is_empty() {
                    let mut lines = Vec::new();
                    if !decls.is_empty() { lines.push(decls); }
                    lines.extend(list.rules.borrow().iter().map(|rule| rule.css_text(owner)).filter(|text| !text.is_empty()));
                    format!("{} {{\n  {}\n}}", selector.borrow(), lines.join("\n  "))
                } else if decls.is_empty() {
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
                    let text = rule.css_text(owner);
                    if text.is_empty() { continue; }
                    out.push_str("  ");
                    out.push_str(&text);
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
    model.edited.set(true);
    let dom = page.dom.borrow();
    if dom.element(owner).is_ok_and(|e| *e.local_name() == *"style") {
        let generation = dom.style_sheet_generation(owner);
        page.set_edited_style_element(owner, text, generation);
    } else {
        page.set_stylesheet_text(owner, &text);
        *model.source.borrow_mut() = text;
    }
}

fn selector_context(parent: &Weak<RuleNode>) -> ParseRelative {
    std::iter::successors(parent.upgrade(), |node| node.parent.borrow().upgrade()).find_map(|node| match &node.kind {
        RuleKind::Style { .. } => Some(ParseRelative::ForNesting),
        RuleKind::Group { bounds: Some(_), .. } => Some(ParseRelative::ForScope),
        _ => None,
    }).unwrap_or(ParseRelative::No)
}

fn insert_into(
    list: &RuleList,
    namespaces: &Namespaces,
    parent: Weak<RuleNode>,
    rule: &str,
    index: usize,
    constructed: bool,
) -> JsResult<()> {
    if index > list.rules.borrow().len() {
        return Err(JsError::dom("IndexSizeError", "The index provided is larger than the number of rules."));
    }
    let mut scoped = namespaces.clone();
    // CSSOM "parse a CSS rule": one rule, and nothing after it but whitespace.
    let invalid = || JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'."));
    let relative = selector_context(&parent);
    let mut parsed = vec![pyro_dom::cssom::parse_one_rule_in_scope(rule, &mut scoped, relative).or_else(|_| {
        if relative == ParseRelative::No { return Err(invalid()); }
        if relative == ParseRelative::ForNesting && matches!(pyro_dom::cssom::parse_one_rule(rule, &mut namespaces.clone()), Ok(parsed) if !matches!(parsed, ParsedRule::Style { .. })) {
            return Err(JsError::dom("HierarchyRequestError", "This rule cannot be nested inside a style rule."));
        }
        let declarations = DeclarationBlock::parse(rule);
        if declarations.is_empty() { return Err(invalid()); }
        Ok(ParsedRule::Declarations(declarations.source_text()))
    })?];
    // CSSOM "insert a CSS rule": past the end is an IndexSizeError; @import and
    // @namespace only belong at the top of a sheet.
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
        let namespace = matches!(parsed[0], ParsedRule::Other { rule_type: 10, .. });
        let import = matches!(parsed[0], ParsedRule::Other { rule_type: IMPORT_RULE, .. });
        if (namespace && rules[index..].iter().any(|rule| matches!(rule.kind, RuleKind::Other { rule_type: IMPORT_RULE, .. })))
            || (import && rules[..index].iter().any(|rule| matches!(rule.kind, RuleKind::Other { rule_type: 10, .. }))) {
            return Err(JsError::dom("HierarchyRequestError", "Imports must come before namespaces."));
        }
        if namespace { check_namespace_edit(&rules)?; }
    }
    // Constructed sheets take no `@import` (CSSOM's insert a CSS rule).
    if constructed && matches!(parsed[0], ParsedRule::Other { rule_type: IMPORT_RULE, .. }) {
        return Err(invalid());
    }
    let node = RuleNode::new(parsed.pop().ok_or_else(invalid)?, parent);
    let mut rules = list.rules.borrow().clone();
    rules.insert(index, node);
    list.replace(rules);
    Ok(())
}

fn check_namespace_edit(rules: &[Rc<RuleNode>]) -> JsResult<()> {
    if rules.iter().all(|rule| matches!(rule.kind, RuleKind::Other { rule_type: IMPORT_RULE | 10, .. })) { return Ok(()); }
    Err(JsError::dom("InvalidStateError", "Cannot change namespaces in a sheet containing other rules."))
}

fn delete_from(list: &RuleList, index: usize) -> JsResult<()> {
    let mut rules = list.rules.borrow().clone();
    if index >= rules.len() {
        return Err(JsError::dom("IndexSizeError", "The index provided is outside the rule list."));
    }
    if matches!(rules[index].kind, RuleKind::Other { rule_type: 10, .. }) { check_namespace_edit(&rules)?; }
    *rules.remove(index).parent.borrow_mut() = Weak::new();
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
                RuleKind::Group { list, .. } | RuleKind::Style { list, .. } if is_attached(&model, node) => list.rules.borrow().clone(),
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
        RuleKind::ViewTransition { .. } => s.wrap::<CssViewTransitionRule>(data, key),
        RuleKind::Style { .. } => s.wrap::<CssStyleRule>(data, key),
        RuleKind::Other { rule_type: 8, .. } => s.wrap::<CssKeyframeRule>(data, key),
        RuleKind::Group { name, .. } if matches!(name.as_str(), "keyframes" | "-webkit-keyframes") => s.wrap::<CssKeyframesRule>(data, key),
        RuleKind::Group { name, .. } if name == "media" => s.wrap::<CssMediaRule>(data, key),
        RuleKind::Group { name, .. } if name == "supports" => s.wrap::<CssSupportsRule>(data, key),
        RuleKind::Group { name, .. } if name == "layer" => s.wrap::<CssLayerBlockRule>(data, key),
        RuleKind::Group { name, .. } if name == "container" => s.wrap::<CssContainerRule>(data, key),
        RuleKind::Group { bounds: Some(_), .. } => s.wrap::<CssScopeRule>(data, key),
        RuleKind::Group { .. } => s.wrap::<CssConditionRule>(data, key),
        RuleKind::Features(_) => s.wrap::<CssFontFeatureValuesRule>(data, key),
        RuleKind::Counter(_) => s.wrap::<CssCounterStyleRule>(data, key),
        RuleKind::Other { rule_type: MARGIN_RULE, .. } => s.wrap::<CssMarginRule>(data, key),
        RuleKind::Other { text, .. } => {
            let text = text.borrow();
            match at_name(&text).as_str() {
                "import" => s.wrap::<CssImportRule>(data, key),
                "font-face" => s.wrap::<CssFontFaceRule>(data, key),
                "page" => s.wrap::<CssPageRule>(data, key),
                "namespace" => s.wrap::<CssNamespaceRule>(data, key),
                "layer" if text.trim_end().ends_with(';') => s.wrap::<CssLayerStatementRule>(data, key),
                "layer" => s.wrap::<CssLayerBlockRule>(data, key),
                "starting-style" => s.wrap::<CssStartingStyleRule>(data, key),
                "position-try" => s.wrap::<CssPositionTryRule>(data, key),
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
    fn media_owner(&self) -> JsResult<MediaOwner> {
        Ok(MediaOwner::Rule(RuleData { owner: self.owner.hold()?, rule_node: self.node()?.clone() }))
    }

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
            RuleKind::Counter(_) => 11.0,
            RuleKind::Features(_) => 14.0,
            RuleKind::Style { .. } => 1.0,
            RuleKind::Group { name, .. } if matches!(name.as_str(), "keyframes" | "-webkit-keyframes") => 7.0,
            RuleKind::Group { name, .. } if name == "media" => 4.0,
            RuleKind::Group { name, .. } if name == "supports" => 12.0,
            RuleKind::Group { .. } | RuleKind::Property(_) | RuleKind::Palette(_) | RuleKind::Declarations(_) | RuleKind::ViewTransition { .. } => 0.0,
            RuleKind::Other { rule_type, .. } => f64::from(*rule_type),
        }))
    });
    // Setting a rule's cssText does nothing (CSSOM).
    def.accessor("cssText", |s, r| Ok(s.string(&r.node()?.css_text(&r.owner))), |_s, _r, _v| Ok(()));
    def.getter("parentRule", |s, r| match r.node()?.parent.borrow().upgrade() {
        Some(parent) => Ok(rule_wrapper(s, &parent, &r.owner)?.value()),
        None => Ok(s.null()),
    });
    // Null once the rule (or the rule holding it) was removed from its sheet.
    def.getter("parentStyleSheet", |s, r| {
        let mut top = r.node()?.clone();
        while let Some(parent) = { let parent = top.parent.borrow().upgrade(); parent } {
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
                Ok(s.wrap::<CssStyleProperties>(StyleData::new(source), Some(key))?.value())
            },
            |_s, r, v| {
                let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
                StyleData::new(source).set_css_text(&dom_string(v)?)
            },
        );
        // CSS Typed OM: the declaration block as a StylePropertyMap.
        def.getter("styleMap", |s, r| {
            let key = IdentityKey(r.node()?.identity.0 | (1 << 55), r.node()?.identity.1);
            let style = match s.lookup_identity(key) {
                Some(o) => o.value(),
                None => {
                    let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
                    s.wrap::<CssStyleProperties>(StyleData::new(source), Some(key))?.value()
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
                let normalized = normalize_relative_selector(&text, &model.namespaces.borrow(), selector_context(&r.node()?.parent.borrow()));
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
            if is_page(r.node()?) {
                let model = r.owner.rules()?;
                edit_page_margins(&model, r.node()?, index, Some(rule.as_str()))?;
                r.owner.commit(&model);
                return Ok(s.number(index as f64));
            }
            let (RuleKind::Group { list, .. } | RuleKind::Style { list, .. }) = &r.node()?.kind else { return Ok(s.undefined()) };
            let model = r.owner.rules()?;
            let namespaces = model.namespaces.borrow().clone();
            insert_into(list, &namespaces, Rc::downgrade(r.node()?), &rule, index, r.owner.is_constructed())?;
            r.owner.commit(&model);
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, r, a| {
            a.require(1, "CSSGroupingRule.deleteRule")?;
            let index = to_u32(a.get(0))? as usize;
            if is_page(r.node()?) {
                let model = r.owner.rules()?;
                edit_page_margins(&model, r.node()?, index, None)?;
                r.owner.commit(&model);
                return Ok(s.undefined());
            }
            let (RuleKind::Group { list, .. } | RuleKind::Style { list, .. }) = &r.node()?.kind else { return Ok(s.undefined()) };
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

/// Public margin-rule text when the internal declaration source retains a negative zero.
fn canonical_margin_text(text: &str) -> Option<String> {
    if !declarations::may_need_signed_zero_backing(text) { return None; }
    let (name, body) = split_block(text)?;
    let block = DeclarationBlock::parse_in_rule(body, CssRuleType::Margin);
    block.css_for_reparse()?;
    Some(format!("{name} {{ {} }}", block.css_text()))
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
const MARGIN_RULES: &[&str] = pyro_dom::cssom::PAGE_MARGIN_RULES;

/// `CSSRule.type` of a margin rule.
const MARGIN_RULE: u16 = 9;


fn is_page(node: &RuleNode) -> bool {
    matches!(&node.kind, RuleKind::Other { rule_type: 6, .. })
}

/// A margin at-rule as its node keeps it: `@top-left { }`, `@top-left { color: red; }`.
fn margin_rule_text(m: &str) -> String {
    match split_block(m) {
        Some((head, body)) if body.trim().is_empty() => format!("{head} {{ }}"),
        Some((head, body)) => format!("{head} {{ {} }}", body.trim()),
        None => m.to_owned(),
    }
}

/// `insertRule` / `deleteRule` of a `@page` rule: its child rules are its margin rules, kept in its text.
/// `insert` is the rule text to insert at `index`, or `None` to delete the rule at `index`.
fn edit_page_margins(model: &SheetModel, page: &Rc<RuleNode>, index: usize, insert: Option<&str>) -> JsResult<()> {
    let mut nodes = page_margins(model, page);
    match insert {
        Some(rule) => {
            if index > nodes.len() {
                return Err(JsError::dom("IndexSizeError", "The index provided is larger than the number of rules."));
            }
            let rule = rule.trim();
            let syntax = || JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'."));
            if !rule.starts_with('@') || split_block(rule).is_none() || !rule.ends_with('}') {
                return Err(syntax());
            }
            if !MARGIN_RULES.contains(&at_name(rule).as_str()) {
                return Err(JsError::dom("HierarchyRequestError", format!("'{rule}' may not be inserted inside an @page rule.")));
            }
            nodes.insert(index, RuleNode::new(ParsedRule::Other { rule_type: MARGIN_RULE, text: margin_rule_text(rule) }, Rc::downgrade(page)));
        }
        None => {
            if index >= nodes.len() {
                return Err(JsError::dom("IndexSizeError", "The index provided is outside the rule list."));
            }
            *nodes.remove(index).parent.borrow_mut() = Weak::new();
        }
    }
    let texts = nodes.iter().map(|n| match &n.kind { RuleKind::Other { text, .. } => text.borrow().clone(), _ => String::new() }).collect();
    model.margins.borrow_mut().insert((page.identity.0, page.identity.1), (texts, nodes));
    write_page(model, page, None);
    Ok(())
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
                let text = margin_rule_text(m);
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
    let mut parser = cssparser::Parser::new(body);
    loop {
        match parser.next_including_whitespace_and_comments() {
            Ok(cssparser::Token::CurlyBracketBlock | cssparser::Token::Semicolon) => {
                return body[..parser.position().byte_index() - 1].trim().to_owned();
            }
            Err(_) => return body.trim().to_owned(),
            _ => {}
        }
    }
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
        rule_media_list,
        |_s, r, v| MediaList::set(&r.media_owner()?, &dom_string(v)?),
    );
}
fn rule_media_list<'s>(s: &Scope<'s>, rule: &RuleData) -> JsResult<Value<'s>> {
    let node = rule.node()?;
    let key = IdentityKey(node.identity.0 | (1 << 53), node.identity.1);
    if let Some(object) = s.lookup_identity(key) {
        return Ok(object.value());
    }
    Ok(s.wrap::<MediaList>(rule.media_owner()?, Some(key))?.value())
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
        StyleData::new(source).set_css_text(&dom_string(value)?)
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
    Ok(s.wrap::<C>(StyleData::new(source), Some(key))?.value())
}

macro_rules! descriptor_accessors {
    ($def:ident, $(($idl:literal, $css:literal)),* $(,)?) => {$(
        $def.accessor($idl, |s, st| Ok(s.string(&st.get($css))), |_s, st, v| st.set($css, &dom_string(v)?, false));
        if $idl != $css {
            $def.accessor($css, |s, st| Ok(s.string(&st.get($css))), |_s, st, v| st.set($css, &dom_string(v)?, false));
        }
    )*};
}

/// `CSSPageDescriptors`: the declarations of a `@page` rule.
pub struct CssPageDescriptors;
impl JsClass for CssPageDescriptors {
    const NAME: &'static str = "CSSPageDescriptors";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(def: &mut ClassDef<Self>) {
        descriptor_accessors!(def,
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
        StyleData::new(source).set_css_text(&dom_string(v)?)
    });
});

use pyro_dom::cssom::page_selector;

rule_class!(CssPageRule, "CSSPageRule", CssGroupRule, |def| {
    def.accessor("style", rule_style::<CssPageDescriptors>, |_s, r, v| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData::new(source).set_css_text(&dom_string(v)?)
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
rule_class!(CssKeyframeRule, "CSSKeyframeRule", CssRule, |def| {
    def.accessor("keyText", |s, r| Ok(s.string(split_block(&rule_text(r)).map_or("", |(head, _)| head))), |_s, r, value| {
        let selector = pyro_dom::cssom::keyframe_selector(&dom_string(value)?).ok_or_else(|| JsError::dom("SyntaxError", "Invalid keyframe selector"))?;
        if let RuleKind::Other { text, .. } = &r.node()?.kind {
            let body = split_block(&text.borrow()).map(|(_, body)| body.to_owned()).unwrap_or_default();
            *text.borrow_mut() = format!("{selector} {{{body}}}");
            r.owner.commit(&*r.owner.rules()?);
        }
        Ok(())
    });
    def.accessor("style", rule_style::<CssStyleProperties>, |_s, r, value| {
        let source = StyleSource::Rule { owner: r.owner.hold()?, node: r.node()?.clone() };
        StyleData::new(source).set_css_text(&dom_string(value)?)
    });
});

rule_class!(CssKeyframesRule, "CSSKeyframesRule", CssRule, |def| {
    fn group(rule: &RuleData) -> JsResult<(&RefCell<String>, &RuleList)> {
        let RuleKind::Group { condition, list, .. } = &rule.node()?.kind else { unreachable!() };
        Ok((condition, list))
    }
    fn find(list: &RuleList, key: &str) -> Option<usize> {
        let selector = pyro_dom::cssom::keyframe_selector(key)?;
        list.rules.borrow().iter().rposition(|child| matches!(&child.kind, RuleKind::Other { text, .. } if split_block(&text.borrow()).is_some_and(|(head, _)| head == selector)))
    }
    def.accessor("name", |s, r| {
        let text = group(r)?.0.borrow();
        let mut parser = cssparser::Parser::new(&text);
        Ok(s.string(parser.expect_ident_or_string().ok().map_or("", |name| name.as_ref())))
    }, |_s, r, value| {
        *group(r)?.0.borrow_mut() = pyro_dom::cssom::serialize_keyframes_name(&dom_string(value)?);
        r.owner.commit(&*r.owner.rules()?);
        Ok(())
    });
    def.getter("cssRules", |s, r| rule_list(s, r.node()?.identity, &r.owner, Some(r.node()?.clone())));
    def.getter("length", |s, r| Ok(s.number(group(r)?.1.rules.borrow().len() as f64)));
    def.indexed(|s, r, index| group(r)?.1.rules.borrow().get(index as usize).map(|child| rule_wrapper(s, child, &r.owner).map(|obj| obj.value())).transpose());
    def.method("appendRule", 1, |s, r, args| {
        if let Some(parsed) = pyro_dom::cssom::parse_one_keyframe(&arg_string(&args, 0, "CSSKeyframesRule.appendRule")?) {
            let list = group(r)?.1;
            let mut children = list.rules.borrow().clone();
            children.push(RuleNode::new(parsed, Rc::downgrade(r.node()?)));
            list.replace(children);
            r.owner.commit(&*r.owner.rules()?);
        }
        Ok(s.undefined())
    });
    def.method("findRule", 1, |s, r, args| {
        let list = group(r)?.1;
        let selector = arg_string(&args, 0, "CSSKeyframesRule.findRule")?;
        match find(list, &selector) {
            Some(index) => Ok(rule_wrapper(s, &list.rules.borrow()[index], &r.owner)?.value()),
            None => Ok(s.null()),
        }
    });
    def.method("deleteRule", 1, |s, r, args| {
        let list = group(r)?.1;
        if let Some(index) = find(list, &arg_string(&args, 0, "CSSKeyframesRule.deleteRule")?) {
            delete_from(list, index)?;
            r.owner.commit(&*r.owner.rules()?);
        }
        Ok(s.undefined())
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
fn feature_rule(data: &RuleData) -> JsResult<&RefCell<FeatureRule>> {
    match &data.node()?.kind {
        RuleKind::Features(rule) => Ok(rule),
        _ => unreachable!(),
    }
}

fn commit_feature_edit(data: &RuleData) -> JsResult<()> {
    let model = data.owner.rules()?;
    if is_attached(&model, &data.rule_node) { data.owner.commit(&model); }
    Ok(())
}

fn feature_map<'s, const BLOCK: usize>(s: &Scope<'s>, data: &RuleData) -> JsResult<Value<'s>> {
    let node = data.node()?;
    let key = IdentityKey(node.identity.0 | ((BLOCK as u64 + 1) << 40), node.identity.1);
    if let Some(obj) = s.lookup_identity(key) { return Ok(obj.value()); }
    let rule = RuleData { owner: data.owner.hold()?, rule_node: node.clone() };
    Ok(s.wrap::<CssFontFeatureValuesMap>(FeatureMapData { rule, block: BLOCK }, Some(key))?.value())
}

rule_class!(CssFontFeatureValuesRule, "CSSFontFeatureValuesRule", CssRule, |def| {
    def.accessor("fontFamily", |s, d| Ok(s.string(&feature_rule(d)?.borrow().family)), |_s, d, value| {
        let text = dom_string(value)?;
        if feature_rule(d)?.borrow_mut().set_family(&text) { commit_feature_edit(d)?; }
        Ok(())
    });
    def.getter("swash", feature_map::<0>);
    def.getter("stylistic", feature_map::<1>);
    def.getter("ornaments", feature_map::<2>);
    def.getter("annotation", feature_map::<3>);
    def.getter("characterVariant", feature_map::<4>);
    def.getter("styleset", feature_map::<5>);
});

pub struct FeatureMapData {
    rule: RuleData,
    block: usize,
}

pub(crate) struct CssFontFeatureValuesMap;
impl JsClass for CssFontFeatureValuesMap {
    const NAME: &'static str = "CSSFontFeatureValuesMap";
    type Data = FeatureMapData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("size", |s, d| Ok(s.number(feature_rule(&d.rule)?.borrow().maps[d.block].len() as f64)));
        def.method("get", 1, |s, d, args| {
            args.require(1, "CSSFontFeatureValuesMap.get")?;
            let key = dom_string(args.get(0))?;
            let rule = feature_rule(&d.rule)?.borrow();
            let Some(values) = rule.maps[d.block].get(&key) else { return Ok(s.undefined()) };
            feature_indexes(s, values)
        });
        def.method("has", 1, |s, d, args| {
            args.require(1, "CSSFontFeatureValuesMap.has")?;
            let key = dom_string(args.get(0))?;
            Ok(s.boolean(feature_rule(&d.rule)?.borrow().maps[d.block].get(&key).is_some()))
        });
        def.method("delete", 1, |s, d, args| {
            args.require(1, "CSSFontFeatureValuesMap.delete")?;
            let key = dom_string(args.get(0))?;
            let removed = feature_rule(&d.rule)?.borrow_mut().maps[d.block].delete(&key);
            if removed { commit_feature_edit(&d.rule)?; }
            Ok(s.boolean(removed))
        });
        def.method("clear", 0, |s, d, _| {
            if feature_rule(&d.rule)?.borrow_mut().maps[d.block].clear() { commit_feature_edit(&d.rule)?; }
            Ok(s.undefined())
        });
        // install.js performs WebIDL union conversion before reaching this mutator.
        def.method("set", 2, |s, d, args| {
            args.require(2, "CSSFontFeatureValuesMap.set")?;
            let key = dom_string(args.get(0))?;
            let seq = args.get(1).as_object().ok_or_else(|| JsError::Type("Expected feature indexes".into()))?;
            let count = to_u32(seq.get("length")?)?;
            let limit = match d.block { 5 => u32::MAX, 4 => 2, _ => 1 };
            if count == 0 || count > limit {
                return Err(JsError::dom("InvalidAccessError", "Invalid number of font feature indexes."));
            }
            let values = (0..count).map(|i| to_u32(seq.get_index(i)?)).collect::<JsResult<Vec<_>>>()?;
            if feature_rule(&d.rule)?.borrow_mut().maps[d.block].set(key, values) { commit_feature_edit(&d.rule)?; }
            Ok(s.undefined())
        });
        // Consumed and hidden by the iterator prototype hook.
        def.method("nextEntry", 1, |s, d, args| {
            let cursor = args.get(0).to_number()? as u64;
            let rule = feature_rule(&d.rule)?.borrow();
            let Some((next, key, values)) = rule.maps[d.block].next(cursor) else { return Ok(s.undefined()) };
            let mut entry = pyro_js::RootedVec::new();
            entry.push(s, s.number(next as f64));
            entry.push(s, s.string(key));
            entry.push(s, feature_indexes(s, values)?);
            Ok(s.array(&entry.iter(s).collect::<Vec<_>>())?.value())
        });
    }
}

fn feature_indexes<'s>(s: &Scope<'s>, values: &[u32]) -> JsResult<Value<'s>> {
    Ok(s.array(&values.iter().map(|&v| s.number(v as f64)).collect::<Vec<_>>())?.value())
}

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
        StyleData::new(source).set_css_text(&dom_string(v)?)
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
        StyleData::new(source).set_css_text(&dom_string(v)?)
    });
});

pub struct CssPositionTryDescriptors;
impl JsClass for CssPositionTryDescriptors {
    const NAME: &'static str = "CSSPositionTryDescriptors";
    type Data = StyleData;
    type Parent = CssStyleDeclaration;
    fn define(def: &mut ClassDef<Self>) {
        descriptor_accessors!(def,
            ("margin", "margin"), ("marginTop", "margin-top"), ("marginRight", "margin-right"),
            ("marginBottom", "margin-bottom"), ("marginLeft", "margin-left"),
            ("marginBlock", "margin-block"), ("marginBlockStart", "margin-block-start"), ("marginBlockEnd", "margin-block-end"),
            ("marginInline", "margin-inline"), ("marginInlineStart", "margin-inline-start"), ("marginInlineEnd", "margin-inline-end"),
            ("inset", "inset"), ("top", "top"), ("right", "right"), ("bottom", "bottom"), ("left", "left"),
            ("insetBlock", "inset-block"), ("insetBlockStart", "inset-block-start"), ("insetBlockEnd", "inset-block-end"),
            ("insetInline", "inset-inline"), ("insetInlineStart", "inset-inline-start"), ("insetInlineEnd", "inset-inline-end"),
            ("width", "width"), ("minWidth", "min-width"), ("maxWidth", "max-width"),
            ("height", "height"), ("minHeight", "min-height"), ("maxHeight", "max-height"),
            ("blockSize", "block-size"), ("minBlockSize", "min-block-size"), ("maxBlockSize", "max-block-size"),
            ("inlineSize", "inline-size"), ("minInlineSize", "min-inline-size"), ("maxInlineSize", "max-inline-size"),
            ("placeSelf", "place-self"), ("alignSelf", "align-self"), ("justifySelf", "justify-self"),
            ("positionAnchor", "position-anchor"), ("positionArea", "position-area"),
        );
    }
}
rule_class!(CssViewTransitionRule, "CSSViewTransitionRule", CssRule, |def| {
    def.getter("navigation", |s, rule| match &rule.node()?.kind {
        RuleKind::ViewTransition { navigation, .. } => Ok(s.string(navigation)),
        _ => unreachable!(),
    });
    def.getter_this("types", |s, rule, this| {
        let RuleKind::ViewTransition { types, .. } = &rule.node()?.kind else { unreachable!() };
        if let Some(array) = s.get_hidden(this, "viewTransitionTypes")? { return Ok(array); }
        let array = s.frozen_array(&types.iter().map(|name| s.string(name)).collect::<Vec<_>>())?;
        s.set_hidden(this, "viewTransitionTypes", array)?;
        Ok(array)
    });
});

/// `CSSRule.type` of `@import`.
const IMPORT_RULE: u16 = 3;

pub struct CssCounterStyleRule;
impl JsClass for CssCounterStyleRule {
    const NAME: &'static str = "CSSCounterStyleRule";
    type Data = RuleData;
    type Parent = CssRule;
    fn define(def: &mut ClassDef<Self>) {
        fn counter(data: &RuleData) -> JsResult<&RefCell<CounterRule>> {
            match &data.node()?.kind { RuleKind::Counter(rule) => Ok(rule), _ => unreachable!() }
        }
        fn edit(data: &RuleData, update: impl FnOnce(&mut CounterRule) -> bool) -> JsResult<()> {
            let changed = update(&mut counter(data)?.borrow_mut());
            if changed {
                if let Ok(model) = data.owner.rules() { data.owner.commit(&model); }
            }
            Ok(())
        }
        def.accessor("name", |scope, data| Ok(scope.string(&counter(data)?.borrow().name())), |_, data, value| {
            let value = dom_string(value)?;
            edit(data, |rule| rule.set_name(&value))
        });
        macro_rules! descriptor {
            ($attribute:literal, $id:ident) => {
                def.accessor($attribute, |scope, data| Ok(scope.string(&counter(data)?.borrow().descriptor(CounterDescriptor::$id))), |_, data, value| {
                    let value = dom_string(value)?;
                    edit(data, |rule| rule.set_descriptor(CounterDescriptor::$id, &value))
                });
            };
        }
        descriptor!("system", System);
        descriptor!("symbols", Symbols);
        descriptor!("additiveSymbols", AdditiveSymbols);
        descriptor!("negative", Negative);
        descriptor!("prefix", Prefix);
        descriptor!("suffix", Suffix);
        descriptor!("range", Range);
        descriptor!("pad", Pad);
        descriptor!("speakAs", SpeakAs);
        descriptor!("fallback", Fallback);
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
    fn font_descriptors(&self, text: &str) -> pyro_dom::font_face::DescriptorBlock {
        let base = match self {
            SheetOwner::Node(hold, _) => {
                let dom = hold.page.dom.borrow();
                let base = dom.document_base_url(hold.page.document()).unwrap_or("about:blank");
                hold.page.layout().style().sheet_base_url(&dom, hold.id, base).unwrap_or_else(|| base.to_owned())
            }
            SheetOwner::Constructed(sheet) => sheet.import.as_ref().map(|import| import.href.clone()).or_else(|| {
                Some(sheet.page.upgrade()?.dom.borrow().document_data(sheet.document).ok()?.url.to_string())
            }).unwrap_or_else(|| "about:blank".into()),
        };
        pyro_dom::font_face::DescriptorBlock::parse(text, &base)
    }
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
                || matches!(&r.kind, RuleKind::Group { list, .. } | RuleKind::Style { list, .. } if within(&list.rules.borrow(), node))
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

use pyro_dom::cssom::media_queries;

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
                    RuleKind::Features(_) | RuleKind::Counter(_) | RuleKind::Style { .. } | RuleKind::Property(_) | RuleKind::Palette(_) | RuleKind::Declarations(_) | RuleKind::ViewTransition { .. } => return Ok(()),
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
            let parsed = media_queries(&arg_string(&a, 0, "MediaList.appendMedium")?);
            let [medium] = parsed.as_slice() else { return Ok(s.undefined()) };
            let mut queries = media_queries(&MediaList::text(sh));
            if !queries.contains(medium) {
                queries.push(medium.clone());
                MediaList::set(sh, &queries.join(", "))?;
            }
            Ok(s.undefined())
        });
        def.method("deleteMedium", 1, |s, sh, a| {
            let parsed = media_queries(&arg_string(&a, 0, "MediaList.deleteMedium")?);
            let [medium] = parsed.as_slice() else { return Ok(s.undefined()) };
            let mut queries = media_queries(&MediaList::text(sh));
            let before = queries.len();
            queries.retain(|q| q != medium);
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
    page.dom.borrow().is_connected(node).unwrap_or(false) && sheet_flag(page, node, SheetGeneration::now(page, node)).get()
}

/// pyro: sets `disabled` of `node`'s current sheet object (`HTMLStyleElement.disabled`).
pub(crate) fn set_current_sheet_disabled(page: &PageDom, node: NodeId, disabled: bool) {
    if !page.dom.borrow().is_connected(node).unwrap_or(false) {
        return;
    }
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
            SheetOwner::Node(h, _) if sh.owner.is_current() && h.page.dom.borrow().is_connected(h.id).js()? => {
                Ok(crate::node::wrap_node(s, &h.page, h.id)?.value())
            }
            _ => Ok(s.null()),
        });
        def.getter("parentStyleSheet", |s, sh| match &sh.owner {
            SheetOwner::Constructed(c) => match &c.import {
                Some(i) if is_attached(&*i.rule.owner.rules()?, i.rule.node()?) => Ok(i.rule.owner.wrapper(s)?.value()),
                _ => Ok(s.null()),
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
                    SheetOwner::Constructed(c) => match &c.import {
                        Some(import) => return rule_media_list(s, &import.rule),
                        None => IdentityKey((1 << 56) | (1 << 53), c.id),
                    },
                };
                if let Some(o) = s.lookup_identity(key) {
                    return Ok(o.value());
                }
                Ok(s.wrap::<MediaList>(MediaOwner::Sheet(sh.owner.hold()?), Some(key))?.value())
            },
            |_s, sh, v| {
                if let SheetOwner::Constructed(c) = &sh.owner {
                    if let Some(import) = &c.import {
                        return MediaList::set(&import.rule.media_owner()?, &dom_string(v)?);
                    }
                }
                MediaList::set(&MediaOwner::Sheet(sh.owner.hold()?), &dom_string(v)?)
            },
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
    // pyro: a `<style>` whose type is not CSS has no sheet (HTML "update a style block").
    if n.page().dom.borrow().style_type_not_css(n.id) {
        return Ok(s.null());
    }
    // pyro: nor one CSP blocked (`style-src-elem`, as Chrome).
    crate::csp::resolve_inline_styles(n.page());
    if n.page().dom.borrow().style_element_blocked(n.id) {
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
        // pyro: a `<style>` CSP blocks has no sheet, from the moment it was inserted or changed.
        crate::csp::resolve_inline_styles(&root.page);
        let dom = root.page.dom.borrow();
        // A disconnected shadow root's `<style>`s have no sheets.
        if !dom.is_connected(root.id).unwrap_or(false) {
            return Vec::new();
        }
        let Ok(nodes) = dom.descendants(root.id) else { return Vec::new() };
        nodes
            .filter(|&n| {
                dom.element(n).is_ok_and(|e| match &*e.name.local {
                    // pyro: a `<style>` whose type is not CSS has no sheet.
                    "style" => {
                        (e.is_html() || e.namespace() == &markup5ever::ns!(svg))
                            && pyro_dom::Dom::style_type_is_css(e.attr("type"))
                            // pyro: nor one CSP blocked (`style-src-elem`).
                            && !dom.style_element_blocked(n)
                    }
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
    engine.register::<CssKeyframeRule>();
    engine.register::<CssNamespaceRule>();
    engine.register::<CssFontFeatureValuesRule>();
    engine.register::<CssFontFeatureValuesMap>();
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
    fn declaration_token_boundaries() {
        let declarations = parse_declarations(r#"CONTENT: "\"}/*;"; /* ; ' */ --Mixed: [a;b] ! /* ; */ important; color: blue;"#);
        assert_eq!(declarations.len(), 3);
        assert_eq!((&*declarations[0].name, &*declarations[0].value), ("content", r#""\"}/*;""#));
        assert_eq!((&*declarations[1].name, &*declarations[1].value, declarations[1].important), ("--Mixed", "[a;b]", true));
        assert_eq!(declarations[2].value, "blue");
        let escaped = parse_declarations(r"--a\,b: value;");
        assert_eq!(escaped[0].name, "--a,b");
        assert_eq!(parse_declarations(&serialize_declarations(&escaped)), escaped);
    }

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
