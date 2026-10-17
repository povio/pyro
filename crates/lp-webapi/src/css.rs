//! `CSSStyleDeclaration` for inline styles (`element.style`) backed by the
//! `style` attribute, and a minimal `CSSStyleSheet` for `<style>`/`<link>`.
//!
//! Shorthand expansion (e.g. `margin` -> `margin-top`) is not modelled:
//! declarations are kept as written. Computed styles come from lp-dom's style
//! engine via `getComputedStyle`.

use std::rc::Rc;

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
    el: NodeHold,
}

impl StyleData {
    fn decls(&self) -> Vec<Declaration> {
        let dom = self.el.page.dom.borrow();
        parse_declarations(dom.get_attribute(self.el.id, "style").ok().flatten().unwrap_or(""))
    }
    fn write(&self, decls: &[Declaration]) -> JsResult<()> {
        self.el.page.dom.borrow_mut().set_attribute(self.el.id, "style", &serialize_declarations(decls)).js()
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
        def.getter("parentRule", |s, _st| Ok(s.null()));
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
    s.wrap::<CssStyleDeclaration>(StyleData { el: NodeHold::new(&page, id)? }, Some(key))
}

// ----- stylesheets ------------------------------------------------------------

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
            let mut rules = sh.owner.page.sheet_rules(sh.owner.id);
            if index > rules.len() {
                return Err(JsError::dom("IndexSizeError", "The index provided is larger than the number of rules."));
            }
            if !rule.contains('{') && !rule.trim_start().starts_with('@') {
                return Err(JsError::dom("SyntaxError", format!("Failed to parse the rule '{rule}'.")));
            }
            rules.insert(index, rule);
            sh.owner.page.set_sheet_rules(sh.owner.id, rules);
            Ok(s.number(index as f64))
        });
        def.method("deleteRule", 1, |s, sh, a| {
            let index = to_u32(a.get(0))? as usize;
            let mut rules = sh.owner.page.sheet_rules(sh.owner.id);
            if index >= rules.len() {
                return Err(JsError::dom("IndexSizeError", "The index provided is outside the rule list."));
            }
            rules.remove(index);
            sh.owner.page.set_sheet_rules(sh.owner.id, rules);
            Ok(s.undefined())
        });
        def.method("replaceSync", 1, |s, sh, a| {
            let text = arg_string(&a, 0, "CSSStyleSheet.replaceSync")?;
            sh.owner.page.set_sheet_rules(sh.owner.id, vec![text]);
            Ok(s.undefined())
        });
    }
}

fn rules_value<'s>(s: &Scope<'s>, sh: &SheetData) -> JsResult<Value<'s>> {
    let rules = sh.owner.page.sheet_rules(sh.owner.id);
    let mut vals = Vec::with_capacity(rules.len());
    for r in &rules {
        let o = s.object();
        o.set("cssText", s.string(r))?;
        o.set("type", s.number(1.0))?;
        vals.push(o.value());
    }
    Ok(s.array(&vals)?.value())
}

pub(crate) fn element_sheet<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let connected = n.page().dom.borrow().is_connected(n.id).js()?;
    if !connected {
        return Ok(s.null());
    }
    let base = n.page().node_identity(n.id);
    let key = IdentityKey(base.0 | (1 << 61) | (23 << 48), base.1);
    if let Some(o) = s.lookup_identity(key) {
        return Ok(o.value());
    }
    let page: Rc<PageDom> = n.page().clone();
    Ok(s.wrap::<CssStyleSheet>(SheetData { owner: NodeHold::new(&page, n.id)? }, Some(key))?.value())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CssStyleDeclaration>();
    engine.register::<CssStyleSheet>();
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
