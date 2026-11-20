//! Positional HTML parsing methods on Element, ShadowRoot and NonDocumentTypeChildNode.

use pyro_dom::NodeType;
use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, Scope, Value};

use crate::node::NodeData;
use crate::trusted_types;
use crate::util::DomResultExt;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Position {
    Before,
    After,
    Replace,
    Append,
    Prepend,
}

pub(crate) fn define_sibling_methods<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    crate::streaming_html::siblings(def);
    def.method("beforeHTMLUnsafe", 1, |s, n, a| insert(s, n, a, Position::Before));
    def.method("afterHTMLUnsafe", 1, |s, n, a| insert(s, n, a, Position::After));
    def.method("replaceWithHTMLUnsafe", 1, |s, n, a| insert(s, n, a, Position::Replace));
}

pub(crate) fn define_parent_methods<C: JsClass<Data = NodeData>>(def: &mut ClassDef<C>) {
    crate::streaming_html::parents(def);
    def.method("appendHTMLUnsafe", 1, |s, n, a| insert(s, n, a, Position::Append));
    def.method("prependHTMLUnsafe", 1, |s, n, a| insert(s, n, a, Position::Prepend));
}

fn insert<'s>(s: &Scope<'s>, n: &NodeData, a: Args<'s>, position: Position) -> JsResult<Value<'s>> {
    let page = n.page();
    let shadow = page.dom.borrow().is_shadow_root(n.id).js()?;
    let sink = match position {
        Position::Before => "Node beforeHTMLUnsafe",
        Position::After => "Node afterHTMLUnsafe",
        Position::Replace => "Node replaceWithHTMLUnsafe",
        Position::Append if shadow => "ShadowRoot appendHTMLUnsafe",
        Position::Append => "Element appendHTMLUnsafe",
        Position::Prepend if shadow => "ShadowRoot prependHTMLUnsafe",
        Position::Prepend => "Element prependHTMLUnsafe",
    };
    a.require(1, sink)?;
    // WebIDL converts the HTML union before it reads the options dictionary.
    // Applying the default policy is a separate step, after options vetting.
    let input = trusted_types::HtmlInput::convert(s, a.get(0))?;
    let (html, run_scripts) = input.for_positional(s, page, a.get(1), sink)?;
    let (target, fragment, before) = {
        let mut dom = page.dom.borrow_mut();
        let target = match position {
            Position::Append | Position::Prepend => dom.template_contents(n.id).js()?.unwrap_or(n.id),
            _ => {
                let Some(parent) = dom.parent(n.id).js()? else { return Ok(s.undefined()) };
                if dom.template_contents(parent).js()?.is_some() {
                    return Err(JsError::dom("HierarchyRequestError", "The parent is a template element."));
                }
                parent
            }
        };
        let context = match dom.node_type(target).js()? {
            NodeType::Element => {
                let el = dom.element(target).js()?;
                if el.is_html() && el.local_name() == &markup5ever::local_name!("html") {
                    return Err(JsError::dom("HierarchyRequestError", "The target is an html element."));
                }
                target
            }
            _ => dom
                .fragment_host(target)
                .js()?
                .ok_or_else(|| JsError::dom("HierarchyRequestError", "HTML insertion requires an element context."))?,
        };
        let before = match position {
            Position::Prepend => dom.first_child(target).js()?,
            Position::After => dom.next_sibling(n.id).js()?,
            Position::Before | Position::Replace => Some(n.id),
            Position::Append => None,
        };
        let fragment = pyro_dom::parse::parse_modern_html_fragment(&mut dom, context, &html, run_scripts).js()?;
        (target, fragment, before)
    };
    if position == Position::Replace {
        // Blur handlers may run JS and collect detached nodes.
        let _keep_alive = crate::collections::NodeHolds::new(page, [target, fragment])?;
        crate::input::blur_before_removal(page, n.id, false);
        page.dom.borrow_mut().replace_child(target, fragment, n.id).js()?;
    } else {
        page.dom.borrow_mut().insert_before(target, fragment, before).js()?;
    }
    Ok(s.undefined())
}
