//! `CSSStyleDeclaration` for inline styles (`element.style`) backed by the
//! `style` attribute, and `CSSStyleSheet` for `<style>`/`<link>`, whose rules
//! are parsed from the sheet text when a script first asks for them.
//!
//! Shorthand expansion (e.g. `margin` -> `margin-top`) is not modelled:
//! declarations are kept as written. Computed styles come from lp-dom's style
//! engine via `getComputedStyle`.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

use lp_dom::cssom::{normalize_selector, parse_rules, Namespaces, ParsedRule};
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
pub fn parse_declarations(text: &str) -> Vec<Declaration> {
    let mut out: Vec<Declaration> = Vec::new();
    let mut depth = 0i32;
    let mut quote: Option<char> = None;
    let mut start = 0;
    let bytes: Vec<(usize, char)> = text.char_indices().collect();
    let mut segments = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let (pos, c) = bytes[i];
        match quote {
            Some(q) => {
                if c == '\\' {
                    i += 1;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '(' => depth += 1,
                ')' => depth -= 1,
                ';' if depth <= 0 => {
                    segments.push(&text[start..pos]);
                    start = pos + 1;
                }
                _ => {}
            },
        }
        i += 1;
    }
    segments.push(&text[start..]);
    for seg in segments {
        let seg = strip_comments(seg);
        let Some((name, value)) = seg.split_once(':') else { continue };
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let name = if name.starts_with("--") { name.to_owned() } else { name.to_ascii_lowercase() };
        let mut value = value.trim().to_owned();
        let mut important = false;
        if let Some(idx) = value.to_ascii_lowercase().rfind("!important") {
            if value[idx + 10..].trim().is_empty() {
                value.truncate(idx);
                value = value.trim().to_owned();
                important = true;
            }
        }
        if value.is_empty() {
            continue;
        }
        out.retain(|d| d.name != name);
        out.push(Declaration { name, value, important });
    }
    out
}

fn strip_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
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

fn known_property(name: &str) -> bool {
    // Property names accepted through named access: anything that looks like
    // a CSS property in camelCase. Methods/attributes on the prototype win
    // because the named hook does not override builtins.
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric())
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

pub struct StyleData {
    source: StyleSource,
}

/// What a CSSStyleDeclaration reads and writes.
enum StyleSource {
    /// An element's `style` attribute.
    Inline(NodeHold),
    /// The declaration block of a style rule of `owner`'s sheet.
    Rule { owner: NodeHold, node: Rc<RuleNode> },
}

impl StyleData {
    fn decls(&self) -> Vec<Declaration> {
        match &self.source {
            StyleSource::Inline(el) => {
                let dom = el.page.dom.borrow();
                parse_declarations(dom.get_attribute(el.id, "style").ok().flatten().unwrap_or(""))
            }
            StyleSource::Rule { node, .. } => match &node.kind {
                RuleKind::Style { block, .. } => parse_declarations(&block.borrow()),
                _ => Vec::new(),
            },
        }
    }
    fn write(&self, decls: &[Declaration]) -> JsResult<()> {
        match &self.source {
            StyleSource::Inline(el) => {
                el.page.dom.borrow_mut().set_attribute(el.id, "style", &serialize_declarations(decls)).js()
            }
            StyleSource::Rule { owner, node } => {
                if let RuleKind::Style { block, .. } = &node.kind {
                    *block.borrow_mut() = serialize_declarations(decls);
                    commit_edit(&owner.page, owner.id, &sheet_model(&owner.page, owner.id));
                }
                Ok(())
            }
        }
    }
    fn get(&self, name: &str) -> String {
        self.decls().into_iter().find(|d| d.name == name).map(|d| d.value).unwrap_or_default()
    }
    fn set(&self, name: &str, value: &str, important: bool) -> JsResult<()> {
        let mut decls = self.decls();
        let value = value.trim();
        if value.is_empty() {
            decls.retain(|d| d.name != name);
        } else if let Some(d) = decls.iter_mut().find(|d| d.name == name) {
            d.value = value.to_owned();
            d.important = important;
        } else {
            decls.push(Declaration { name: name.to_owned(), value: value.to_owned(), important });
        }
        self.write(&decls)
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
            |s, st| Ok(s.string(&serialize_declarations(&st.decls()))),
            |_s, st, v| {
                let decls = parse_declarations(&dom_string(v)?);
                st.write(&decls)
            },
        );
        def.getter("length", |s, st| Ok(s.number(st.decls().len() as f64)));
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
            Ok(s.string(st.decls().get(i).map(|d| d.name.as_str()).unwrap_or("")))
        });
        def.indexed(|s, st, i| Ok(st.decls().get(i as usize).map(|d| s.string(&d.name))));
        def.method("getPropertyValue", 1, |s, st, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.getPropertyValue")?;
            let name = if name.starts_with("--") { name } else { name.to_ascii_lowercase() };
            Ok(s.string(&st.get(&name)))
        });
        def.method("getPropertyPriority", 1, |s, st, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.getPropertyPriority")?.to_ascii_lowercase();
            let imp = st.decls().iter().any(|d| d.name == name && d.important);
            Ok(s.string(if imp { "important" } else { "" }))
        });
        def.method("setProperty", 2, |s, st, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.setProperty")?;
            let name = if name.starts_with("--") { name } else { name.to_ascii_lowercase() };
            let value = null_to_empty(a.get(1))?;
            let prio = if a.get(2).is_undefined() { String::new() } else { dom_string(a.get(2))? };
            if !prio.is_empty() && !prio.eq_ignore_ascii_case("important") {
                return Ok(s.undefined());
            }
            st.set(&name, &value, !prio.is_empty())?;
            Ok(s.undefined())
        });
        def.method("removeProperty", 1, |s, st, a| {
            let name = arg_string(&a, 0, "CSSStyleDeclaration.removeProperty")?.to_ascii_lowercase();
            let old = st.get(&name);
            st.set(&name, "", false)?;
            Ok(s.string(&old))
        });
        def.named(false, |s, st, name| {
            if !known_property(name) {
                return Ok(None);
            }
            Ok(Some(s.string(&st.get(&camel_to_css(name)))))
        });
        def.named_setter(|_s, st, name, v| {
            if !known_property(name) {
                return Ok(false);
            }
            let value = if v.is_null() { String::new() } else { dom_string(v)? };
            st.set(&camel_to_css(name), &value, false)?;
            Ok(true)
        });
        def.property_names(|_s, st| Ok(st.decls().into_iter().map(|d| d.name).collect()));
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
        // Its child text content, as the style engine reads it.
        return dom.child_text_content(owner).into_owned();
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
                let source = StyleSource::Rule {
                    owner: NodeHold::new(&r.owner.page, r.owner.id)?,
                    node: r.node.clone(),
                };
                Ok(s.wrap::<CssStyleDeclaration>(StyleData { source }, Some(key))?.value())
            },
            |_s, r, v| {
                let decls = parse_declarations(&dom_string(v)?);
                let source = StyleSource::Rule {
                    owner: NodeHold::new(&r.owner.page, r.owner.id)?,
                    node: r.node.clone(),
                };
                StyleData { source }.write(&decls)
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
