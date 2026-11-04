//! Custom elements: `customElements.define/get/whenDefined/upgrade`,
//! construction and upgrade of existing elements.
//!
//! Gap (documented): connected/disconnected/attributeChanged/adopted
//! reactions are only run when an element is upgraded while connected
//! (`connectedCallback`); later DOM insertions/removals/attribute changes
//! do not enqueue reactions yet.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, RealmId, Scope, Value};
use rustc_hash::FxHashMap;

use crate::node::{wrap_new_node, wrap_node};
use crate::page::PageDom;
use crate::target::page_of;
use crate::util::*;

#[derive(Default)]
pub struct Registry {
    defs: FxHashMap<(RealmId, String), Persistent>,
    waiting: FxHashMap<(RealmId, String), Vec<lp_js::Deferred>>,
    /// Elements being upgraded (construction stack).
    stack: Vec<NodeId>,
}

thread_local! {
    static REGISTRIES: RefCell<FxHashMap<u64, Registry>> = RefCell::new(FxHashMap::default());
}

fn with_registry<R>(page: &PageDom, f: impl FnOnce(&mut Registry) -> R) -> R {
    REGISTRIES.with(|r| f(r.borrow_mut().entry(page.id()).or_default()))
}

fn valid_name(name: &str) -> bool {
    const RESERVED: &[&str] = &[
        "annotation-xml",
        "color-profile",
        "font-face",
        "font-face-src",
        "font-face-uri",
        "font-face-format",
        "font-face-name",
        "missing-glyph",
    ];
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name.contains('-')
        && !name.chars().any(|c| c.is_ascii_uppercase())
        && !RESERVED.contains(&name)
}

/// `HTMLElement` constructor (only valid via a defined custom element class).
pub(crate) fn construct_html_element<'s>(s: &Scope<'s>, new_target: Object<'s>) -> JsResult<Object<'s>> {
    let page = page_of(s)?;
    let realm = s.realm_id();
    let def_name = with_registry(&page, |r| {
        r.defs
            .iter()
            .find(|((rid, _), ctor)| *rid == realm && ctor.get(s).strict_equals(new_target.value()))
            .map(|((_, n), _)| n.clone())
    });
    let Some(name) = def_name else { return Err(JsError::Type("Illegal constructor".into())) };
    let upgrading = with_registry(&page, |r| r.stack.pop());
    let proto = new_target.get("prototype")?;
    match upgrading {
        Some(el) => {
            let obj = wrap_node(s, &page, el)?;
            obj.set_prototype(proto);
            Ok(obj)
        }
        None => {
            let doc = page.document();
            let el = page.dom.borrow_mut().create_element(doc, &name).js()?;
            page.dom.borrow_mut().set_custom_defined(el).js()?;
            wrap_new_node::<crate::html::HtmlElement>(s, &page, el, new_target)
        }
    }
}

/// Upgrades `el` with `ctor` (construction stack + `new ctor()`).
fn upgrade<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, ctor: Value<'s>) -> JsResult<()> {
    with_registry(page, |r| r.stack.push(el));
    let result = ctor.as_object().ok_or_else(|| JsError::Type("not a constructor".into()))?.construct(&[]);
    with_registry(page, |r| {
        if r.stack.last() == Some(&el) {
            r.stack.pop();
        }
    });
    result?;
    page.dom.borrow_mut().set_custom_defined(el).js()?;
    if page.dom.borrow().is_connected(el).js()? {
        let obj = wrap_node(s, page, el)?;
        if let Some(cb) = obj.get("connectedCallback")?.as_object().filter(|f| f.is_function()) {
            if let Err(e) = cb.call(Some(obj), &[]) {
                s.report_exception(&e);
            }
        }
    }
    Ok(())
}

/// `document.createElement` result: constructs defined custom elements.
pub(crate) fn create_element_value<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId) -> JsResult<Value<'s>> {
    let name = page.dom.borrow().element(el).js()?.local_name().to_string();
    let ctor = with_registry(page, |r| r.defs.get(&(s.realm_id(), name)).map(|p| p.get(s)));
    if let Some(ctor) = ctor {
        upgrade(s, page, el, ctor)?;
    }
    Ok(wrap_node(s, page, el)?.value())
}

pub struct CustomElementRegistry;
impl JsClass for CustomElementRegistry {
    const NAME: &'static str = "CustomElementRegistry";
    type Data = Rc<PageDom>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("define", 2, |s, page, a| {
            a.require(2, "CustomElementRegistry.define")?;
            let name = dom_string(a.get(0))?;
            let ctor = a.get(1);
            if !ctor.as_object().is_some_and(|c| c.is_constructor()) {
                return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': The provided value is not a constructor.".into()));
            }
            if !valid_name(&name) {
                return Err(JsError::dom("SyntaxError", format!("Failed to execute 'define' on 'CustomElementRegistry': \"{name}\" is not a valid custom element name")));
            }
            let key = (s.realm_id(), name.clone());
            let exists = with_registry(page, |r| r.defs.contains_key(&key) || r.defs.iter().any(|(k, c)| k.0 == key.0 && c.get(s).strict_equals(ctor)));
            if exists {
                return Err(JsError::dom("NotSupportedError", format!("Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry")));
            }
            let disabled = ctor.as_object().expect("constructor").get("disabledFeatures")?;
            if let Some(list) = disabled.as_object() {
                let len = list.get("length")?.to_number()? as u32;
                for i in 0..len {
                    if dom_string(list.get_index(i)?)? == "shadow" {
                        page.dom.borrow_mut().disable_shadow_for(&name);
                    }
                }
            }
            with_registry(page, |r| r.defs.insert(key.clone(), Persistent::new(s, ctor)));
            // Upgrade existing elements of the document, in tree order.
            let existing: Vec<NodeId> = {
                let dom = page.dom.borrow();
                let doc = page.document();
                dom.descendants(doc)
                    .map(|d| d.filter(|&n| dom.element(n).is_ok_and(|e| e.is_html() && *e.local_name() == *name)).collect())
                    .unwrap_or_default()
            };
            for el in existing {
                if let Err(e) = upgrade(s, page, el, ctor) {
                    s.report_exception(&e);
                }
            }
            let waiting = with_registry(page, |r| r.waiting.remove(&key).unwrap_or_default());
            for d in waiting {
                d.resolve(s, ctor)?;
            }
            Ok(s.undefined())
        });
        def.method("get", 1, |s, page, a| {
            let name = dom_string(a.get(0))?;
            Ok(with_registry(page, |r| r.defs.get(&(s.realm_id(), name)).map(|p| p.get(s))).unwrap_or(s.undefined()))
        });
        def.method("getName", 1, |s, page, a| {
            let ctor = a.get(0);
            let name = with_registry(page, |r| {
                r.defs
                    .iter()
                    .find(|(k, c)| k.0 == s.realm_id() && c.get(s).strict_equals(ctor))
                    .map(|(k, _)| k.1.clone())
            });
            Ok(name.map_or(s.null(), |n| s.string(&n)))
        });
        def.method("whenDefined", 1, |s, page, a| {
            let name = dom_string(a.get(0))?;
            let d = s.deferred()?;
            if !valid_name(&name) {
                d.reject(
                    s,
                    JsError::dom("SyntaxError", format!("\"{name}\" is not a valid custom element name")).to_value(s),
                )?;
                return Ok(d.promise.get(s));
            }
            let key = (s.realm_id(), name);
            let existing = with_registry(page, |r| r.defs.get(&key).map(|p| p.get(s)));
            let promise = d.promise.get(s);
            match existing {
                Some(ctor) => d.resolve(s, ctor)?,
                None => with_registry(page, |r| r.waiting.entry(key).or_default().push(d)),
            }
            Ok(promise)
        });
        def.method("upgrade", 1, |s, page, a| {
            let root = crate::node::node_arg(s, a.get(0), "CustomElementRegistry.upgrade")?;
            let candidates: Vec<(NodeId, String)> = {
                let dom = page.dom.borrow();
                dom.descendants(root)
                    .map(|d| d.filter_map(|n| dom.element(n).ok().map(|e| (n, e.local_name().to_string()))).collect())
                    .unwrap_or_default()
            };
            for (el, name) in candidates {
                let ctor = with_registry(page, |r| r.defs.get(&(s.realm_id(), name)).map(|p| p.get(s)));
                if let Some(c) = ctor {
                    upgrade(s, page, el, c)?;
                }
            }
            Ok(s.undefined())
        });
    }
}

/// `window.customElements` (one registry object per realm).
pub fn registry<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = lp_js::IdentityKey(page.id() | (1 << 58), 0);
    crate::window::same_object(s, page, key, || s.wrap::<CustomElementRegistry>(page.clone(), Some(key)))
}

/// Drops a page's registry state (page teardown).
pub fn forget_page(page_id: u64) {
    REGISTRIES.with(|r| {
        r.borrow_mut().remove(&page_id);
    });
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CustomElementRegistry>();
}
