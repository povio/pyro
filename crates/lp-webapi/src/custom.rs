//! Custom elements: `customElements.define/get/whenDefined/upgrade`,
//! construction and upgrade of existing elements.
//!
//! Callback reactions: the DOM queues `connectedCallback`,
//! `disconnectedCallback` and `connectedMoveCallback` reactions as custom
//! elements are inserted, removed and moved (`lp_dom::custom`), and
//! [`run_reactions`] runs them when the DOM call that queued them returns
//! (the native callback hook, the `[CEReactions]` point). The callbacks are
//! the ones the prototype had when the element was defined.
//!
//! Gap (documented): `attributeChangedCallback` and `adoptedCallback`
//! reactions are not enqueued, and the parser does not upgrade elements.

use std::cell::RefCell;
use std::rc::Rc;

use lp_dom::NodeId;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, RealmId, Scope, Value};
use rustc_hash::FxHashMap;

use crate::node::{wrap_new_node, wrap_node};
use crate::page::PageDom;
use crate::target::page_of;
use crate::util::*;

/// The lifecycle callbacks of a definition, read from the prototype by
/// `define` (HTML: "lifecycle callbacks").
#[derive(Default)]
struct Callbacks {
    connected: Option<Persistent>,
    disconnected: Option<Persistent>,
    connected_move: Option<Persistent>,
}

#[derive(Default)]
pub struct Registry {
    defs: FxHashMap<(RealmId, String), Persistent>,
    callbacks: FxHashMap<(RealmId, String), Callbacks>,
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

thread_local! {
    /// Pages whose reaction queue is being run (a callback's own DOM calls
    /// add to the queue the running loop empties).
    static RUNNING: RefCell<Vec<u64>> = const { RefCell::new(Vec::new()) };
}

/// Runs the custom element callback reactions the DOM queued for `page`'s
/// document, in order, until none is left. Called when a native DOM call
/// returns to script and after each task; a held DOM borrow defers them to
/// the next such point.
pub(crate) fn run_reactions(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    let pending = page.dom.try_borrow().is_ok_and(|dom| dom.has_custom_reactions_for(page.document()));
    if !pending || RUNNING.with(|r| r.borrow().contains(&page.id())) {
        return;
    }
    RUNNING.with(|r| r.borrow_mut().push(page.id()));
    struct Done(u64);
    impl Drop for Done {
        fn drop(&mut self) {
            RUNNING.with(|r| r.borrow_mut().retain(|&id| id != self.0));
        }
    }
    let _done = Done(page.id());
    loop {
        let batch = {
            let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
            dom.take_custom_reactions(page.document())
        };
        if batch.is_empty() {
            return;
        }
        for (el, reaction) in batch {
            if page.is_disposed() {
                return;
            }
            run_reaction(page, el, reaction);
        }
    }
}

/// Calls the callback of one reaction on `el` with no arguments. A move
/// without `connectedMoveCallback` calls `disconnectedCallback` and then
/// `connectedCallback` (HTML "enqueue a custom element callback reaction").
fn run_reaction(page: &Rc<PageDom>, el: NodeId, reaction: lp_dom::custom::CustomReaction) {
    use lp_dom::custom::CustomReaction;
    let Some(name) = page.dom.borrow().element(el).ok().map(|e| e.local_name().to_string()) else { return };
    // The definition of the element's name, in the realm that defined it.
    let realms: Vec<RealmId> =
        with_registry(page, |r| r.callbacks.keys().filter(|(_, n)| *n == name).map(|(realm, _)| *realm).collect());
    for realm_id in realms {
        let realm = page.realms.borrow().get(&realm_id).and_then(|r| r.upgrade());
        let Some(realm) = realm.filter(|r| !r.is_disposed()) else { continue };
        let Ok(s) = realm.scope() else { continue };
        let callbacks: Vec<Value<'_>> = with_registry(page, |r| {
            let Some(c) = r.callbacks.get(&(realm_id, name.clone())) else { return Vec::new() };
            let pick = |p: &Option<Persistent>| p.as_ref().map(|p| p.get(&s));
            match reaction {
                CustomReaction::Connected => pick(&c.connected).into_iter().collect(),
                CustomReaction::Disconnected => pick(&c.disconnected).into_iter().collect(),
                CustomReaction::Moved => match pick(&c.connected_move) {
                    Some(moved) => vec![moved],
                    None => pick(&c.disconnected).into_iter().chain(pick(&c.connected)).collect(),
                },
            }
        });
        if callbacks.is_empty() {
            return;
        }
        let Ok(obj) = wrap_node(&s, page, el) else { return };
        for callback in callbacks {
            let Some(f) = callback.as_object() else { continue };
            if let Err(e) = f.call(Some(obj), &[]) {
                s.report_exception(&e);
            }
        }
        return;
    }
}

/// HTML "try to upgrade an element" for the elements that became connected
/// while not custom (`lp_dom::upgrade`): each is upgraded with the definition
/// its node document's window has for its name, whichever realm inserted it.
/// Called where the callback reactions run: when a native DOM call returns
/// to script and after each task.
pub(crate) fn run_upgrades(page: &Rc<PageDom>) {
    if page.is_disposed() {
        return;
    }
    loop {
        let batch = {
            let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
            dom.take_upgrade_candidates()
        };
        if batch.is_empty() {
            return;
        }
        for el in batch {
            try_upgrade(page, el);
        }
    }
}

fn try_upgrade(page: &Rc<PageDom>, el: NodeId) {
    let (name, doc) = {
        let dom = page.dom.borrow();
        if !dom.awaits_upgrade(el) {
            return;
        }
        let (Ok(element), Ok(doc)) = (dom.element(el), dom.node_document(el)) else { return };
        (element.local_name().to_string(), doc)
    };
    // "Look up a custom element definition": the registry is the one of the
    // node document's window (the page's own world before an isolated one).
    let Some(owner) = document_page(page, doc) else { return };
    let main = owner.main_world_realm().map(|r| r.id());
    let realm_id = with_registry(&owner, |r| {
        let mut realms: Vec<RealmId> = r.defs.keys().filter(|(_, n)| *n == name).map(|(realm, _)| *realm).collect();
        realms.sort_by_key(|id| Some(*id) != main);
        realms.first().copied()
    });
    let Some(realm_id) = realm_id else { return };
    let realm = owner.realms.borrow().get(&realm_id).and_then(|r| r.upgrade());
    let Some(realm) = realm.filter(|r| !r.is_disposed()) else { return };
    let Ok(s) = realm.scope() else { return };
    let Some(ctor) = with_registry(&owner, |r| r.defs.get(&(realm_id, name)).map(|p| p.get(&s))) else { return };
    if let Err(e) = upgrade(&s, &owner, el, ctor) {
        s.report_exception(&e);
    }
}

/// The bindings of the window that displays `doc`: `page`'s own, or those of
/// another document of its frame tree.
fn document_page(page: &Rc<PageDom>, doc: NodeId) -> Option<Rc<PageDom>> {
    if page.document() == doc {
        return Some(page.clone());
    }
    let realm = page.main_world_realm()?;
    let scope = realm.scope().ok()?;
    page.sibling_realms(&scope).iter().find_map(|sibling| {
        let s = sibling.scope().ok()?;
        page_of(&s).ok().filter(|p| p.document() == doc && !p.is_disposed())
    })
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
            // The lifecycle callbacks are those of the prototype at definition.
            let proto = ctor.as_object().expect("constructor").get("prototype")?;
            let Some(proto) = proto.as_object() else {
                return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': constructor prototype is not an object".into()));
            };
            let lifecycle = |name: &str| -> JsResult<Option<Persistent>> {
                let value = proto.get(name)?;
                if value.is_undefined() {
                    return Ok(None);
                }
                if !value.as_object().is_some_and(|f| f.is_function()) {
                    return Err(JsError::Type(format!("Failed to execute 'define' on 'CustomElementRegistry': {name} is not a function")));
                }
                Ok(Some(Persistent::new(s, value)))
            };
            let callbacks = Callbacks {
                connected: lifecycle("connectedCallback")?,
                disconnected: lifecycle("disconnectedCallback")?,
                connected_move: lifecycle("connectedMoveCallback")?,
            };
            let disabled = ctor.as_object().expect("constructor").get("disabledFeatures")?;
            if let Some(list) = disabled.as_object() {
                let len = list.get("length")?.to_number()? as u32;
                for i in 0..len {
                    if dom_string(list.get_index(i)?)? == "shadow" {
                        page.dom.borrow_mut().disable_shadow_for(&name);
                    }
                }
            }
            with_registry(page, |r| {
                r.callbacks.insert(key.clone(), callbacks);
                r.defs.insert(key.clone(), Persistent::new(s, ctor))
            });
            // From now on an element of this name is upgraded when it connects.
            page.dom.borrow_mut().enable_custom_upgrades();
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
