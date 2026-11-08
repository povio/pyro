//! Custom elements: `customElements.define/get/whenDefined/upgrade`,
//! construction and upgrade of existing elements.
//!
//! Callback reactions: the DOM queues `connectedCallback`,
//! `disconnectedCallback`, `connectedMoveCallback`, `adoptedCallback`,
//! `attributeChangedCallback` (for the attributes a definition observes)
//! and the form-associated callbacks as custom elements change
//! (`lp_dom::custom`), and [`run_reactions`] runs them when the DOM call
//! that queued them returns (the native callback hook, the `[CEReactions]`
//! point). The callbacks are the ones the prototype had when the element
//! was defined, and `observedAttributes` is read once, at definition.
//!
//! The main document parser constructs elements whose name is defined
//! synchronously ([`ParserHooks`], HTML "create an element for a token"),
//! and runs the reactions its insertions queue before it parses on.
//!
//! Not supported: customized built-in elements (`is`), scoped registries,
//! and `formStateRestoreCallback` (there is no session history or autofill
//! restore to call it).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use lp_dom::custom::{CustomReaction, ParserConstruct, ParserCustomElements};
use lp_dom::{LocalName, NodeId, QualName};
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
    adopted: Option<Persistent>,
    attribute_changed: Option<Persistent>,
    form_associated: Option<Persistent>,
    form_reset: Option<Persistent>,
    form_disabled: Option<Persistent>,
    form_state_restore: Option<Persistent>,
}

/// A custom element definition of one realm (HTML "custom element definition").
struct Definition {
    ctor: Persistent,
    callbacks: Callbacks,
    /// `observedAttributes` (empty without an `attributeChangedCallback`).
    observed: Vec<LocalName>,
    form_associated: bool,
    disable_internals: bool,
}

/// The custom element registries of a page's realms, kept on the page
/// (`PageDom::custom_elements`) and keyed by the local name's atom, so
/// that `createElement` finds a definition (or that there is none) with an
/// atom hash and no string.
#[derive(Default)]
pub(crate) struct Registry {
    defs: FxHashMap<(RealmId, LocalName), Definition>,
    waiting: FxHashMap<(RealmId, LocalName), Vec<lp_js::Deferred>>,
    /// Elements being upgraded (construction stack).
    stack: Vec<NodeId>,
    /// Elements whose upgrade constructor is running (custom element state
    /// "precustomized"), for `attachInternals`.
    constructing: Vec<NodeId>,
    /// The parser hooks were given to the DOM.
    parser_hooks: bool,
    /// Realms whose `CustomStateSet.prototype` has its setlike members.
    setlike_realms: Vec<RealmId>,
    /// HTML "throw-on-dynamic-markup-insertion counter" of the page's
    /// document: the parser is constructing a custom element.
    throw_on_dynamic_markup: Cell<u32>,
}

impl Registry {
    fn ctor<'s>(&self, s: &Scope<'s>, realm: RealmId, name: &LocalName) -> Option<Value<'s>> {
        if self.defs.is_empty() {
            return None;
        }
        self.defs.get(&(realm, name.clone())).map(|d| d.ctor.get(s))
    }
}

fn with_registry<R>(page: &PageDom, f: impl FnOnce(&mut Registry) -> R) -> R {
    f(&mut page.custom_elements.borrow_mut())
}

/// Forgets the definitions of a disposed realm (their constructors and
/// callbacks are that realm's) and the promises waiting in it.
pub(crate) fn realm_disposed(page: &PageDom, realm: RealmId) {
    let (defs, waiting) = {
        let mut r = page.custom_elements.borrow_mut();
        let defs: Vec<Definition> = {
            let keys: Vec<_> = r.defs.keys().filter(|k| k.0 == realm).cloned().collect();
            keys.iter().filter_map(|k| r.defs.remove(k)).collect()
        };
        let keys: Vec<_> = r.waiting.keys().filter(|k| k.0 == realm).cloned().collect();
        let waiting: Vec<_> = keys.iter().filter_map(|k| r.waiting.remove(k)).collect();
        r.setlike_realms.retain(|&id| id != realm);
        (defs, waiting)
    };
    // Dropped outside the borrow: releasing a root may release wrappers.
    drop(defs);
    drop(waiting);
}

/// `document.open/write/writeln/close` throw while the parser constructs a
/// custom element of the page's document.
pub(crate) fn check_dynamic_markup(page: &PageDom, doc: NodeId) -> JsResult<()> {
    let throws =
        doc == page.document() && page.custom_elements.try_borrow().is_ok_and(|r| r.throw_on_dynamic_markup.get() > 0);
    if throws {
        return Err(JsError::dom(
            "InvalidStateError",
            "Dynamic markup insertion is not allowed while the parser constructs a custom element.",
        ));
    }
    Ok(())
}

/// Whether the definition of `name` disables internals (`disabledFeatures`
/// has "internals"); `None` when no realm of `page` defined it.
pub(crate) fn internals_disabled(page: &PageDom, name: &LocalName) -> Option<bool> {
    let r = page.custom_elements.borrow();
    r.defs.iter().find(|((_, n), _)| n == name).map(|(_, d)| d.disable_internals)
}

/// Whether this is the realm's first `CustomStateSet` (its prototype's
/// setlike members are still to add).
pub(crate) fn first_state_set(page: &PageDom, realm: RealmId) -> bool {
    let mut r = page.custom_elements.borrow_mut();
    if r.setlike_realms.contains(&realm) {
        return false;
    }
    r.setlike_realms.push(realm);
    true
}

/// Whether `el`'s constructor is running (upgrade or construction).
pub(crate) fn is_being_constructed(page: &PageDom, el: NodeId) -> bool {
    page.custom_elements.borrow().constructing.contains(&el)
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

/// What an element's definition makes it: (observes attributes,
/// form-associated).
fn definition_flags(page: &PageDom, realm: RealmId, name: &LocalName) -> (bool, bool) {
    let r = page.custom_elements.borrow();
    r.defs.get(&(realm, name.clone())).map_or((false, false), |d| (!d.observed.is_empty(), d.form_associated))
}

/// `HTMLElement` constructor (only valid via a defined custom element class).
pub(crate) fn construct_html_element<'s>(s: &Scope<'s>, new_target: Object<'s>) -> JsResult<Object<'s>> {
    let page = page_of(s)?;
    let realm = s.realm_id();
    let def_name = with_registry(&page, |r| {
        r.defs
            .iter()
            .find(|((rid, _), d)| *rid == realm && d.ctor.get(s).strict_equals(new_target.value()))
            .map(|((_, n), _)| n.clone())
    });
    let Some(name) = def_name else { return Err(JsError::Type("Illegal constructor".into())) };
    let upgrading = with_registry(&page, |r| r.stack.pop());
    let proto = new_target.get_atom("prototype")?;
    match upgrading {
        Some(el) => {
            let obj = wrap_node(s, &page, el)?;
            obj.set_prototype(proto);
            Ok(obj)
        }
        None => {
            // `new C()` (and the parser's construction): a new element in
            // state "custom", its constructor running.
            let doc = page.document();
            let (observes, face) = definition_flags(&page, realm, &name);
            let el = {
                let mut dom = page.dom.borrow_mut();
                let el = dom.create_element(doc, &name).js()?;
                dom.prepare_custom_element(el, observes, face).js()?;
                dom.set_custom_defined(el).js()?;
                if face {
                    dom.set_form_associated(el).js()?;
                }
                el
            };
            wrap_new_node::<crate::html::HtmlElement>(s, &page, el, new_target)
        }
    }
}

/// Upgrades `el` with `ctor` (HTML "upgrade an element"): the construction
/// stack + `new ctor()`, then `attributeChangedCallback` for the observed
/// attributes it had, `connectedCallback` when it was connected, and the
/// form-associated callbacks.
fn upgrade<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, ctor: Value<'s>) -> JsResult<()> {
    upgrade_with(s, page, el, ctor, |name| {
        JsError::Type(format!("Failed to upgrade \"{name}\": the constructor did not return the element it upgraded.").into())
    })
}

/// The parser's synchronous construction of `el` (HTML "create an element
/// for a token"): the upgrade of the element it made for the token, whose
/// constructor must return that element (else `NotSupportedError`).
fn construct_for_parser<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, ctor: Value<'s>) -> JsResult<()> {
    upgrade_with(s, page, el, ctor, |name| {
        JsError::dom(
            "NotSupportedError",
            format!("The result of constructing a custom element \"{name}\" must be the element the parser created."),
        )
    })
}

fn upgrade_with<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    el: NodeId,
    ctor: Value<'s>,
    mismatch: impl FnOnce(&LocalName) -> JsError,
) -> JsResult<()> {
    let realm = s.realm_id();
    let name = page.dom.borrow().element(el).js()?.local_name().clone();
    let (observed, face) = {
        let r = page.custom_elements.borrow();
        r.defs.get(&(realm, name.clone())).map_or((Vec::new(), false), |d| (d.observed.clone(), d.form_associated))
    };
    // The reactions the upgrade enqueues before it constructs.
    let (attributes, connected) = {
        let mut dom = page.dom.borrow_mut();
        let e = dom.element(el).js()?;
        let attributes: Vec<(LocalName, String, String)> = e
            .attrs
            .iter()
            .filter(|a| observed.contains(&a.name.local))
            .map(|a| (a.name.local.clone(), a.name.ns.to_string(), a.value.to_string()))
            .collect();
        let connected = dom.is_connected(el).js()?;
        dom.prepare_custom_element(el, !observed.is_empty(), face).js()?;
        (attributes, connected)
    };
    with_registry(page, |r| {
        r.stack.push(el);
        r.constructing.push(el);
    });
    let result = ctor.as_object().ok_or_else(|| JsError::Type("not a constructor".into()))?.construct(&[]);
    with_registry(page, |r| {
        if r.stack.last() == Some(&el) {
            r.stack.pop();
        }
        r.constructing.retain(|&c| c != el);
    });
    // "If SameValue(constructResult, element) is false, throw".
    let result = result.and_then(|made| {
        if crate::node::node_arg(s, made.value(), "upgrade").ok() == Some(el) {
            Ok(())
        } else {
            Err(mismatch(&name))
        }
    });
    if let Err(e) = result {
        let _ = page.dom.borrow_mut().set_custom_failed(el);
        return Err(e);
    }
    page.dom.borrow_mut().set_custom_defined(el).js()?;
    let callbacks: (Option<Value<'s>>, Option<Value<'s>>) = {
        let r = page.custom_elements.borrow();
        let d = r.defs.get(&(realm, name.clone()));
        let pick = |p: Option<&Persistent>| p.map(|p| p.get(s));
        (
            pick(d.and_then(|d| d.callbacks.attribute_changed.as_ref())),
            pick(d.and_then(|d| d.callbacks.connected.as_ref())),
        )
    };
    let obj = wrap_node(s, page, el)?;
    if let Some(cb) = callbacks.0.and_then(|c| c.as_object()) {
        for (local, ns, value) in attributes {
            let ns = if ns.is_empty() { s.null() } else { s.string(&ns) };
            if let Err(e) = cb.call(Some(obj), &[s.string(&local), s.null(), s.string(&value), ns]) {
                s.report_exception(&e);
            }
        }
    }
    if connected {
        if let Some(cb) = callbacks.1.and_then(|c| c.as_object()) {
            if let Err(e) = cb.call(Some(obj), &[]) {
                s.report_exception(&e);
            }
        }
    }
    if face {
        // Queues formAssociatedCallback / formDisabledCallback.
        page.dom.borrow_mut().set_form_associated(el).js()?;
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

/// Calls the callback of one reaction on `el`. A move without
/// `connectedMoveCallback` calls `disconnectedCallback` and then
/// `connectedCallback` (HTML "enqueue a custom element callback reaction").
fn run_reaction(page: &Rc<PageDom>, el: NodeId, reaction: CustomReaction) {
    let Some(name) = page.dom.borrow().element(el).ok().map(|e| e.local_name().clone()) else { return };
    // The definition of the element's name, in the realm that defined it.
    let realms: Vec<RealmId> =
        with_registry(page, |r| r.defs.keys().filter(|(_, n)| *n == name).map(|(realm, _)| *realm).collect());
    for realm_id in realms {
        let realm = page.realms.borrow().get(&realm_id).and_then(|r| r.upgrade());
        let Some(realm) = realm.filter(|r| !r.is_disposed()) else { continue };
        let Ok(s) = realm.scope() else { continue };
        let callbacks: Vec<Value<'_>> = with_registry(page, |r| {
            let Some(d) = r.defs.get(&(realm_id, name.clone())) else { return Vec::new() };
            let c = &d.callbacks;
            let pick = |p: &Option<Persistent>| p.as_ref().map(|p| p.get(&s));
            match &reaction {
                CustomReaction::Connected => pick(&c.connected).into_iter().collect(),
                CustomReaction::Disconnected => pick(&c.disconnected).into_iter().collect(),
                CustomReaction::Moved => match pick(&c.connected_move) {
                    Some(moved) => vec![moved],
                    None => pick(&c.disconnected).into_iter().chain(pick(&c.connected)).collect(),
                },
                CustomReaction::Adopted { .. } => pick(&c.adopted).into_iter().collect(),
                // Another realm's definition of the name may observe more.
                CustomReaction::AttributeChanged(change) if d.observed.contains(&change.local) => {
                    pick(&c.attribute_changed).into_iter().collect()
                }
                CustomReaction::AttributeChanged(_) => Vec::new(),
                CustomReaction::FormAssociated(_) => pick(&c.form_associated).into_iter().collect(),
                CustomReaction::FormReset => pick(&c.form_reset).into_iter().collect(),
                CustomReaction::FormDisabled(_) => pick(&c.form_disabled).into_iter().collect(),
            }
        });
        if callbacks.is_empty() {
            return;
        }
        let Ok(obj) = wrap_node(&s, page, el) else { return };
        let node = |id: Option<NodeId>| -> Value<'_> {
            id.and_then(|id| wrap_node(&s, page, id).ok()).map_or(s.null(), |o| o.value())
        };
        let args: Vec<Value<'_>> = match &reaction {
            CustomReaction::Adopted { old, new } => vec![node(Some(*old)), node(Some(*new))],
            CustomReaction::AttributeChanged(change) => {
                let opt = |v: &Option<String>| v.as_deref().map_or(s.null(), |v| s.string(v));
                let ns = if change.ns.is_empty() { s.null() } else { s.string(&change.ns) };
                vec![s.string(&change.local), opt(&change.old), opt(&change.new), ns]
            }
            CustomReaction::FormAssociated(form) => vec![node(*form)],
            CustomReaction::FormDisabled(disabled) => vec![s.boolean(*disabled)],
            _ => Vec::new(),
        };
        for callback in callbacks {
            let Some(f) = callback.as_object() else { continue };
            if let Err(e) = f.call(Some(obj), &args) {
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
        (element.local_name().clone(), doc)
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
    let Some(ctor) = with_registry(&owner, |r| r.ctor(&s, realm_id, &name)) else { return };
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
/// `el` was just made in `doc`, a document of `page`.
pub(crate) fn create_element_value<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    doc: NodeId,
    el: NodeId,
) -> JsResult<Value<'s>> {
    let ctor = {
        let registry = page.custom_elements.borrow();
        if registry.defs.is_empty() {
            None
        } else {
            let dom = page.dom.borrow();
            registry.ctor(s, s.realm_id(), dom.element(el).js()?.local_name())
        }
    };
    if let Some(ctor) = ctor {
        // The constructor may have wrapped it (`this`).
        upgrade(s, page, el, ctor)?;
        return Ok(wrap_node(s, page, el)?.value());
    }
    Ok(crate::node::wrap_created_node(s, page, doc, el)?.value())
}

/// DOM "clone a node": a clone of a defined custom element made for the
/// page's document is upgraded at once, connected or not ("create an
/// element" with the synchronous custom elements flag unset enqueues an
/// upgrade reaction, run when the cloning call returns). Free while no
/// element is defined.
pub(crate) fn upgrade_clone(s: &Scope<'_>, page: &Rc<PageDom>, root: NodeId) {
    if page.custom_elements.borrow().defs.is_empty() {
        return;
    }
    let candidates: Vec<(NodeId, LocalName)> = {
        let dom = page.dom.borrow();
        if dom.node_document(root).ok() != Some(page.document()) {
            return;
        }
        dom.descendants(root)
            .map(|d| {
                d.filter_map(|n| {
                    let e = dom.element(n).ok()?;
                    let custom = e.flags
                        & (lp_dom::node::element_flags::CUSTOM_DEFINED | lp_dom::node::element_flags::CUSTOM_FAILED)
                        != 0;
                    (e.is_html() && !custom && e.local_name().contains('-')).then(|| (n, e.local_name().clone()))
                })
                .collect()
            })
            .unwrap_or_default()
    };
    for (el, name) in candidates {
        let Some(ctor) = with_registry(page, |r| r.ctor(s, s.realm_id(), &name)) else { continue };
        if let Err(e) = upgrade(s, page, el, ctor) {
            s.report_exception(&e);
        }
    }
}

/// The main document parser's custom element construction (installed on
/// the DOM with the first definition).
struct ParserHooks {
    page: Weak<PageDom>,
}

impl ParserHooks {
    /// Runs `f` with the document's throw-on-dynamic-markup-insertion
    /// counter raised.
    fn counted<R>(page: &PageDom, f: impl FnOnce() -> R) -> R {
        let counter = |delta: i32| {
            if let Ok(r) = page.custom_elements.try_borrow() {
                r.throw_on_dynamic_markup.set(r.throw_on_dynamic_markup.get().saturating_add_signed(delta));
            }
        };
        counter(1);
        let result = f();
        counter(-1);
        result
    }
}

impl ParserCustomElements for ParserHooks {
    fn construct(&self, el: NodeId, attrs: &[(QualName, String)]) -> ParserConstruct {
        let Some(page) = self.page.upgrade() else { return ParserConstruct::NotDefined };
        if page.is_disposed() {
            return ParserConstruct::NotDefined;
        }
        let name = {
            let dom = page.dom.borrow();
            if dom.node_document(el).ok() != Some(page.document()) {
                return ParserConstruct::NotDefined;
            }
            let Ok(e) = dom.element(el) else { return ParserConstruct::NotDefined };
            e.local_name().clone()
        };
        let Some(realm) = page.main_world_realm().filter(|r| !r.is_disposed()) else {
            return ParserConstruct::NotDefined;
        };
        let Ok(s) = realm.scope() else { return ParserConstruct::NotDefined };
        let Some(ctor) = with_registry(&page, |r| r.ctor(&s, realm.id(), &name)) else {
            return ParserConstruct::NotDefined;
        };
        // "Perform a microtask checkpoint" (no script runs here).
        crate::mutation_observer::checkpoint(&page);
        Self::counted(&page, || {
            let outcome = match construct_for_parser(&s, &page, el, ctor) {
                Ok(()) => {
                    if let Err(e) = page.dom.borrow_mut().append_parser_attributes(el, attrs) {
                        s.report_exception(&dom_err(e));
                    }
                    ParserConstruct::Constructed
                }
                Err(e) => {
                    s.report_exception(&e);
                    ParserConstruct::Failed
                }
            };
            run_reactions(&page);
            run_upgrades(&page);
            outcome
        })
    }

    fn run_reactions(&self) {
        let Some(page) = self.page.upgrade() else { return };
        // Markup insertion from these callbacks would re-enter the running
        // parser, which is not supported: it throws as during construction.
        Self::counted(&page, || {
            run_reactions(&page);
            run_upgrades(&page);
        });
    }
}

/// WebIDL `sequence<DOMString>` of `value` (`what` names the member).
fn string_sequence(s: &Scope<'_>, value: Value<'_>, what: &str) -> JsResult<Vec<String>> {
    let seq = s.sequence(value, what)?;
    let len = seq.get_atom("length")?.to_number()? as u32;
    (0..len).map(|i| dom_string(seq.get_index(i)?)).collect()
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
            let key = (s.realm_id(), LocalName::from(name.as_str()));
            let exists = with_registry(page, |r| {
                r.defs.contains_key(&key) || r.defs.iter().any(|(k, d)| k.0 == key.0 && d.ctor.get(s).strict_equals(ctor))
            });
            if exists {
                return Err(JsError::dom("NotSupportedError", format!("Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry")));
            }
            let constructor = ctor.as_object().expect("constructor");
            // The lifecycle callbacks are those of the prototype at definition.
            let proto = constructor.get_atom("prototype")?;
            let Some(proto) = proto.as_object() else {
                return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': constructor prototype is not an object".into()));
            };
            let lifecycle = |name: &str| -> JsResult<Option<Persistent>> {
                let value = proto.get(name)?;
                if value.is_undefined() {
                    return Ok(None);
                }
                if !value.as_object().is_some_and(|f| f.is_function()) {
                    return Err(JsError::Type(format!("Failed to execute 'define' on 'CustomElementRegistry': {name} is not a function").into()));
                }
                Ok(Some(Persistent::new(s, value)))
            };
            let mut callbacks = Callbacks {
                connected: lifecycle("connectedCallback")?,
                disconnected: lifecycle("disconnectedCallback")?,
                connected_move: lifecycle("connectedMoveCallback")?,
                adopted: lifecycle("adoptedCallback")?,
                attribute_changed: lifecycle("attributeChangedCallback")?,
                ..Callbacks::default()
            };
            let mut observed: Vec<LocalName> = Vec::new();
            if callbacks.attribute_changed.is_some() {
                let list = constructor.get_atom("observedAttributes")?;
                if !list.is_undefined() {
                    for a in string_sequence(s, list, "observedAttributes")? {
                        let a = LocalName::from(a.as_str());
                        if !observed.contains(&a) {
                            observed.push(a);
                        }
                    }
                }
            }
            let mut disable_internals = false;
            let mut disable_shadow = false;
            let disabled = constructor.get_atom("disabledFeatures")?;
            if !disabled.is_undefined() {
                for feature in string_sequence(s, disabled, "disabledFeatures")? {
                    match feature.as_str() {
                        "internals" => disable_internals = true,
                        "shadow" => disable_shadow = true,
                        _ => {}
                    }
                }
            }
            let form_associated = constructor.get_atom("formAssociated")?.to_bool();
            if form_associated {
                callbacks.form_associated = lifecycle("formAssociatedCallback")?;
                callbacks.form_reset = lifecycle("formResetCallback")?;
                callbacks.form_disabled = lifecycle("formDisabledCallback")?;
                callbacks.form_state_restore = lifecycle("formStateRestoreCallback")?;
            }
            if disable_shadow {
                page.dom.borrow_mut().disable_shadow_for(&name);
            }
            let definition = Definition {
                ctor: Persistent::new(s, ctor),
                callbacks,
                observed: observed.clone(),
                form_associated,
                disable_internals,
            };
            let install_hooks = with_registry(page, |r| !std::mem::replace(&mut r.parser_hooks, true));
            let previous = with_registry(page, |r| r.defs.insert(key.clone(), definition));
            drop(previous);
            {
                let mut dom = page.dom.borrow_mut();
                // From now on an element of this name is upgraded when it
                // connects, and the main document parser constructs it.
                dom.enable_custom_upgrades();
                dom.note_custom_definition(&key.1, observed);
                if install_hooks {
                    let hooks = Rc::new(ParserHooks { page: Rc::downgrade(page) });
                    dom.set_parser_custom_elements(page.document(), Some(hooks));
                }
            }
            // Upgrade existing elements of the document, in tree order.
            let existing: Vec<NodeId> = {
                let dom = page.dom.borrow();
                let doc = page.document();
                let undefined = lp_dom::node::element_flags::CUSTOM_DEFINED | lp_dom::node::element_flags::CUSTOM_FAILED;
                dom.descendants(doc)
                    .map(|d| {
                        d.filter(|&n| {
                            dom.element(n).is_ok_and(|e| e.is_html() && *e.local_name() == key.1 && e.flags & undefined == 0)
                        })
                        .collect()
                    })
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
        def.pure_method("get", 1, |s, page, a| {
            let name = with_arg_str(&a, 0, "CustomElementRegistry.get", |name| Ok(LocalName::from(name)))?;
            Ok(with_registry(page, |r| r.ctor(s, s.realm_id(), &name)).unwrap_or(s.undefined()))
        });
        def.pure_method("getName", 1, |s, page, a| {
            let ctor = a.get(0);
            let name = with_registry(page, |r| {
                r.defs
                    .iter()
                    .find(|(k, d)| k.0 == s.realm_id() && d.ctor.get(s).strict_equals(ctor))
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
            let key = (s.realm_id(), LocalName::from(name.as_str()));
            let existing = with_registry(page, |r| r.ctor(s, key.0, &key.1));
            let promise = d.promise.get(s);
            match existing {
                Some(ctor) => d.resolve(s, ctor)?,
                None => with_registry(page, |r| r.waiting.entry(key).or_default().push(d)),
            }
            Ok(promise)
        });
        def.method("upgrade", 1, |s, page, a| {
            let root = crate::node::node_arg(s, a.get(0), "CustomElementRegistry.upgrade")?;
            let candidates: Vec<(NodeId, LocalName)> = {
                let dom = page.dom.borrow();
                let undefined =
                    lp_dom::node::element_flags::CUSTOM_DEFINED | lp_dom::node::element_flags::CUSTOM_FAILED;
                dom.descendants(root)
                    .map(|d| {
                        d.filter_map(|n| {
                            dom.element(n)
                                .ok()
                                .filter(|e| e.flags & undefined == 0)
                                .map(|e| (n, e.local_name().clone()))
                        })
                        .collect()
                    })
                    .unwrap_or_default()
            };
            for (el, name) in candidates {
                let ctor = with_registry(page, |r| r.ctor(s, s.realm_id(), &name));
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

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<CustomElementRegistry>();
}
