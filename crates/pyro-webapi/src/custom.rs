//! Custom elements: `customElements.define/get/whenDefined/upgrade`,
//! construction and upgrade of existing elements.
//!
//! Callback reactions: the DOM queues `connectedCallback`,
//! `disconnectedCallback`, `connectedMoveCallback`, `adoptedCallback`,
//! `attributeChangedCallback` (for the attributes a definition observes)
//! and the form-associated callbacks as custom elements change
//! (`pyro_dom::custom`), and [`run_reactions`] runs them when the DOM call
//! that queued them returns (the native callback hook, the `[CEReactions]`
//! point). The callbacks are the ones the prototype had when the element
//! was defined, and `observedAttributes` is read once, at definition.
//!
//! The main document parser constructs elements whose name is defined
//! synchronously ([`ParserHooks`], HTML "create an element for a token"),
//! and runs the reactions its insertions queue before it parses on.
//!
//! Customized built-in elements (`define(name, C, {extends})`, `<button
//! is=name>`, `createElement("button", {is})`): a definition's local name is
//! the element it extends; an element's is value names its definition
//! (`ElementData::is_value`), and every HTML element interface's constructor
//! runs the HTML element constructor steps.
//!
//! Not supported: `formStateRestoreCallback` (there is no session history or autofill
//! restore to call it).

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use pyro_dom::custom::{CustomReaction, ParserConstruct, ParserCustomElements};
use pyro_dom::{LocalName, NodeId, QualName, RegistryAssociation};
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, RealmId, Scope, Value};
use rustc_hash::{FxHashMap, FxHashSet};

mod association;
mod values;
use values::{PendingDefinition, RegistryValue};
use association::*;
pub(crate) use association::{node_registry, registry_argument};

pub(crate) fn prune_registry_roots(page: &Rc<PageDom>) {
    association::prune_unassociated(page);
}

use crate::node::{wrap_new_node, wrap_node};
use crate::page::PageDom;
use crate::target::page_of;
use crate::util::*;

/// The lifecycle callbacks of a definition, read from the prototype by
/// `define` (HTML: "lifecycle callbacks").
#[derive(Default)]
struct Callbacks {
    connected: Option<RegistryValue>,
    disconnected: Option<RegistryValue>,
    connected_move: Option<RegistryValue>,
    adopted: Option<RegistryValue>,
    attribute_changed: Option<RegistryValue>,
    form_associated: Option<RegistryValue>,
    form_reset: Option<RegistryValue>,
    form_disabled: Option<RegistryValue>,
    form_state_restore: Option<RegistryValue>,
}

/// A custom element definition of one realm (HTML "custom element definition").
struct Definition {
    ctor: RegistryValue,
    callbacks: Callbacks,
    /// `observedAttributes` (empty without an `attributeChangedCallback`).
    observed: Vec<LocalName>,
    form_associated: bool,
    disable_internals: bool,
    disable_shadow: bool,
    /// The element's local name: the definition's name, or the element it extends (a customized
    /// built-in element).
    local_name: LocalName,
}

/// The custom element registries of a page's realms, kept on the page
/// (`PageDom::custom_elements`) and keyed by the local name's atom, so
/// that `createElement` finds a definition (or that there is none) with an
/// atom hash and no string.
#[derive(Default)]
pub(crate) struct Registry {
    defs: FxHashMap<(RegistryKey, LocalName), Rc<Definition>>,
    defining: FxHashSet<RegistryKey>,
    scoped: FxHashMap<u64, ScopedEntry>,
    pinned: FxHashSet<u64>,
    waiting: FxHashMap<(RegistryKey, LocalName), PendingDefinition>,
    /// The parser hooks were given to the DOM.
    parser_hooks: bool,
    /// Realms whose `CustomStateSet.prototype` has its setlike members.
    setlike_realms: Vec<RealmId>,
    /// HTML "throw-on-dynamic-markup-insertion counter" of the page's
    /// document: the parser is constructing a custom element.
    throw_on_dynamic_markup: Cell<u32>,
}

impl Registry {
    fn ctor<'s>(&self, s: &Scope<'s>, realm: RegistryKey, name: &LocalName) -> Option<Value<'s>> {
        if self.defs.is_empty() {
            return None;
        }
        self.defs.get(&(realm, name.clone())).map(|d| d.ctor.get(s))
    }

    /// DOM "look up a custom element definition" in `realm` for an HTML element whose local name
    /// is `local` and whose is value is `is`: the name of the autonomous definition of `local`,
    /// else of the customized built-in definition `is` of `local`.
    fn def_key(&self, realm: RegistryKey, local: &LocalName, is: Option<&LocalName>) -> Option<LocalName> {
        if self.defs.is_empty() {
            return None;
        }
        let of = |name: &LocalName| self.defs.get(&(realm, name.clone())).is_some_and(|d| d.local_name == *local);
        if of(local) {
            return Some(local.clone());
        }
        is.filter(|is| of(is)).cloned()
    }


}

/// `el`'s local name and is value.
fn element_names(page: &PageDom, el: NodeId) -> Option<(LocalName, Option<LocalName>)> {
    let dom = page.dom.borrow();
    let e = dom.element(el).ok()?;
    Some((e.local_name().clone(), e.is_value.clone()))
}

fn with_registry<R>(page: &PageDom, f: impl FnOnce(&mut Registry) -> R) -> R {
    f(&mut page.custom_elements.borrow_mut())
}

/// A detached document's realms may linger while script holds their objects.
/// Scoped definitions and waiters have only traced wrapper-owned references;
/// they stay usable without retaining the old realm through a native root.
pub(crate) fn release_document(page: &PageDom) {
    if page.is_disposed() {
        drop(page.custom_elements.take());
        return;
    }
    let mut state = page.custom_elements.borrow_mut();
    state.defs.retain(|(key, _), _| key.scoped.is_some());
    state.waiting.retain(|(key, _), _| key.scoped.is_some());
    state.setlike_realms.clear();
}

pub(crate) fn realm_disposed(page: &PageDom, realm: RealmId) {
    let lingering = page.is_detached() && !page.is_disposed();
    let remove = |key: RegistryKey| key.realm == realm && (!lingering || key.scoped.is_none());
    let (defs, waiting, scoped) = {
        let mut state = page.custom_elements.borrow_mut();
        let keys: Vec<_> = state.defs.keys().filter(|(key, _)| remove(*key)).cloned().collect();
        let defs: Vec<_> = keys.iter().filter_map(|k| state.defs.remove(k)).collect();
        let keys: Vec<_> = state.waiting.keys().filter(|(key, _)| remove(*key)).cloned().collect();
        let waiting: Vec<_> = keys.iter().filter_map(|k| state.waiting.remove(k)).collect();
        let mut scoped = Vec::new();
        if !lingering {
            state.scoped.retain(|id, entry| {
                if entry.realm == realm { scoped.push(*id); false } else { true }
            });
        }
        state.setlike_realms.retain(|&id| id != realm);
        (defs, waiting, scoped)
    };
    for id in scoped { forget_scoped(id); }
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

/// The definition assigned at construction stays with an element across
/// adoption, independently of its currently associated registry.
fn element_definition(page: &Rc<PageDom>, el: NodeId) -> Option<(Rc<PageDom>, RegistryKey)> {
    let recorded = page.dom.borrow().custom_definition_owner(el).ok()??;
    let scoped = match recorded.registry {
        RegistryAssociation::Scoped(id) => Some(id),
        RegistryAssociation::Default => None,
        RegistryAssociation::Null => return None,
    };
    let owner = if page.id() == recorded.page {
        page.clone()
    } else if let Some(id) = scoped {
        scoped_owner(id)?
    } else {
        page.hooks()?.frame_tree().into_iter().find(|p| p.id() == recorded.page)?
    };
    Some((owner, RegistryKey { realm: RealmId(recorded.realm.try_into().ok()?), scoped }))
}

fn record_definition(dom: &mut pyro_dom::Dom, el: NodeId, owner: &PageDom, key: RegistryKey) -> JsResult<()> {
    dom.set_custom_definition_owner(el, pyro_dom::CustomDefinitionOwner {
        page: owner.id(),
        realm: u64::from(key.realm.0),
        registry: key.association(),
    }).js()
}

pub(crate) fn internals_disabled(page: &Rc<PageDom>, el: NodeId, name: &LocalName) -> Option<bool> {
    let (owner, key) = element_definition(page, el)?;
    let registry = owner.custom_elements.borrow();
    registry.defs.get(&(key, name.clone())).map(|d| d.disable_internals)
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
    CONSTRUCTIONS.with(|frames| frames.borrow().iter().any(|f| f.upgrade.as_ref().is_some_and(|u| u.el == el && Rc::ptr_eq(&page.dom, &u.page.dom))))
}

use pyro_dom::dom::is_valid_custom_element_name as valid_name;

/// HTML's active custom element constructor map and per-definition
/// construction stack. Lookups select the innermost invocation of the same
/// function; an unrelated direct construction falls back to its global
/// registry. A frame remains in place after super() as the already-
/// constructed marker until the invocation returns.
struct ConstructionFrame {
    constructor: Persistent,
    selected: UpgradeDefinition,
    // Only upgrading an existing element pushes onto its definition's
    // construction stack. A fresh createElement call sets the active map.
    upgrade: Option<UpgradeTarget>,
}

struct UpgradeTarget {
    page: Rc<PageDom>,
    el: NodeId,
    consumed: bool,
}

thread_local! {
    static CONSTRUCTIONS: RefCell<Vec<ConstructionFrame>> = const { RefCell::new(Vec::new()) };
}

struct ConstructionGuard;
impl Drop for ConstructionGuard {
    fn drop(&mut self) {
        let frame = CONSTRUCTIONS.with(|frames| frames.borrow_mut().pop());
        drop(frame);
    }
}

pub(crate) fn construct_html_element<'s>(s: &Scope<'s>, new_target: Object<'s>, active: &str) -> JsResult<Object<'s>> {
    let page = page_of(s)?;
    let target = new_target.value().as_raw();
    let pending = CONSTRUCTIONS.with(|frames| {
        frames.borrow().iter().rfind(|f| f.constructor.as_raw() == target).map(|f| f.selected.clone())
    });
    let selection = pending.or_else(|| {
        let key = RegistryKey::global(s.realm_id());
        with_registry(&page, |r| {
            r.defs.iter().find(|((registry, _), d)| *registry == key && d.ctor.get(s).strict_equals(new_target.value()))
                .map(|((_, name), definition)| UpgradeDefinition {
                    owner: page.clone(), key, name: name.clone(), definition: definition.clone(),
                })
        })
    });
    let Some(selected) = selection else { return Err(JsError::Type("Illegal constructor".into())) };
    let name = &selected.name;
    let local_name = &selected.definition.local_name;
    let expected = if local_name == name { "HTMLElement" } else { crate::html::interface_name_for_html(local_name) };
    if active != expected {
        return Err(JsError::Type("Illegal constructor".into()));
    }
    // An inner invocation may change the active registry without changing
    // any definition's stack. A stack entry belongs to the exact definition.
    let target_index = CONSTRUCTIONS.with(|frames| frames.borrow().iter().rposition(|f| {
        f.upgrade.is_some() && Rc::ptr_eq(&f.selected.definition, &selected.definition)
    }));
    if let Some(index) = target_index {
        // Reading the prototype may run author code; retain the original
        // stack position and check its marker only after that read.
        let proto = new_target.get_atom("prototype")?;
        let (node_page, el) = CONSTRUCTIONS.with(|frames| {
            let mut frames = frames.borrow_mut();
            let u = frames[index].upgrade.as_mut().expect("selected upgrade target");
            if std::mem::replace(&mut u.consumed, true) {
                Err(JsError::Type("The custom element has already been constructed.".into()))
            } else { Ok((u.page.clone(), u.el)) }
        })?;
        let obj = wrap_node(s, &node_page, el)?;
        obj.set_prototype(proto);
        return Ok(obj);
    }

    let (observes, face) = (!selected.definition.observed.is_empty(), selected.definition.form_associated);
    let el = {
        let mut dom = page.dom.borrow_mut();
        let el = dom.create_element_raw(page.document(), QualName::new(None, markup5ever::ns!(html), local_name.clone())).js()?;
        if local_name != name {
            dom.set_is_value(el, Some(name.clone())).js()?;
        }
        dom.prepare_custom_element(el, observes, face).js()?;
        dom.set_registry_association(el, selected.key.association()).js()?;
        record_definition(&mut dom, el, &selected.owner, selected.key)?;
        dom.set_custom_defined(el).js()?;
        if face {
            dom.set_form_associated(el).js()?;
        }
        el
    };
    if local_name != name {
        return crate::html::wrap_new_html_element(s, &page, el, new_target);
    }
    wrap_new_node::<crate::html::HtmlElement>(s, &page, el, new_target)
}

/// Upgrades `el` with `ctor` (HTML "upgrade an element"): the construction
/// stack + `new ctor()`, then `attributeChangedCallback` for the observed
/// attributes it had, `connectedCallback` when it was connected, and the
/// form-associated callbacks.
#[derive(Clone)]
struct UpgradeDefinition {
    owner: Rc<PageDom>,
    key: RegistryKey,
    name: LocalName,
    definition: Rc<Definition>,
}

impl UpgradeDefinition {
    fn lookup(owner: Rc<PageDom>, key: RegistryKey, name: LocalName) -> Option<Self> {
        let definition = owner.custom_elements.borrow().defs.get(&(key, name.clone()))?.clone();
        Some(Self { owner, key, name, definition })
    }
}

fn upgrade<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, definition: UpgradeDefinition) -> JsResult<()> {
    upgrade_with(s, page, el, definition, |name| {
        JsError::Type(format!("Failed to upgrade \"{name}\": the constructor did not return the element it upgraded.").into())
    })
}

fn construct_for_parser<'s>(s: &Scope<'s>, page: &Rc<PageDom>, el: NodeId, definition: UpgradeDefinition) -> JsResult<()> {
    upgrade_with(s, page, el, definition, |name| {
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
    definition: UpgradeDefinition,
    mismatch: impl FnOnce(&LocalName) -> JsError,
) -> JsResult<()> {
    // A reaction names its definition when it is enqueued. Earlier constructors
    // can move a later candidate to another document before its reaction runs.
    let UpgradeDefinition { owner, key, name, definition: d } = definition;
    if page.dom.borrow().custom_definition_owner(el).js()?.is_some() {
        return Ok(());
    }
    let (ctor, observed, face) = (d.ctor.get(s), &d.observed, d.form_associated);
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
        record_definition(&mut dom, el, &owner, key)?;
        (attributes, connected)
    };
    let constructor = ctor.as_object().ok_or_else(|| JsError::Type("not a constructor".into()))?;
    let result = {
        CONSTRUCTIONS.with(|frames| frames.borrow_mut().push(ConstructionFrame {
            constructor: Persistent::new(s, ctor),
            selected: UpgradeDefinition { owner: owner.clone(), key, name: name.clone(), definition: d.clone() },
            upgrade: Some(UpgradeTarget { page: page.clone(), el, consumed: false }),
        }));
        let _restore = ConstructionGuard;
        constructor.construct(&[])
    };
    let result = result.and_then(|made| {
        if made.value().strict_equals(wrap_node(s, page, el)?.value()) { Ok(()) } else { Err(mismatch(&name)) }
    });
    if let Err(e) = result {
        let _ = page.dom.borrow_mut().set_custom_failed(el);
        return Err(e);
    }
    page.dom.borrow_mut().set_custom_defined(el).js()?;
    let callbacks: (Option<Value<'s>>, Option<Value<'s>>) = {
        let pick = |p: Option<&RegistryValue>| p.map(|p| p.get(s));
        (pick(d.callbacks.attribute_changed.as_ref()), pick(d.callbacks.connected.as_ref()))
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
    // HTML runs the reactions a callback's DOM calls queue nested in that
    // callback ([CEReactions]); here they run after it, in the next batch, so
    // the batch count is how deep they would nest. A callback that inserts an
    // element whose callback inserts another nests without end; as in a
    // browser it ends with a stack overflow, here at a fixed depth.
    let mut depth = 0;
    loop {
        let batch = {
            let Ok(mut dom) = page.dom.try_borrow_mut() else { return };
            dom.take_custom_reactions(page.document())
        };
        if batch.is_empty() {
            return;
        }
        let overflow = depth >= MAX_REACTION_NESTING;
        depth += 1;
        for (el, reaction) in batch {
            if page.is_disposed() {
                return;
            }
            run_reaction(page, el, reaction, overflow);
        }
    }
}

/// How deep reactions nest (see [`run_reactions`]) before a callback throws
/// a `RangeError` instead of running, as on a browser's stack overflow.
const MAX_REACTION_NESTING: usize = 256;

/// Calls the callback of one reaction on `el`. A move without
/// `connectedMoveCallback` calls `disconnectedCallback` and then
/// `connectedCallback` (HTML "enqueue a custom element callback reaction").
fn run_reaction(page: &Rc<PageDom>, el: NodeId, reaction: CustomReaction, overflow: bool) {
    let Some((local, is)) = element_names(page, el) else { return };
    let Some((owner, key)) = element_definition(page, el) else { return };
    let Some(name) = with_registry(&owner, |r| r.def_key(key, &local, is.as_ref())) else { return };
    let realm = owner.realms.borrow().get(&key.realm).and_then(|r| r.upgrade());
    let Some(realm) = realm.filter(|r| !r.is_disposed() || (key.scoped.is_some() && r.is_lingering())) else { return };
    let Ok(s) = realm.scope() else { return };
    let callbacks: Vec<Value<'_>> = with_registry(&owner, |r| {
        let Some(d) = r.defs.get(&(key, name.clone())) else { return Vec::new() };
        let c = &d.callbacks;
        let pick = |p: &Option<RegistryValue>| p.as_ref().map(|p| p.get(&s));
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
        if overflow {
            s.report_exception(&JsError::Range("Maximum call stack size exceeded.".into()));
            continue;
        }
        if let Err(e) = f.call(Some(obj), &args) {
            s.report_exception(&e);
        }
    }
}

/// HTML "try to upgrade an element" for the elements that became connected
/// while not custom (`pyro_dom::upgrade`): each is upgraded with the definition
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
            dom.take_upgrade_reactions()
        };
        if batch.is_empty() {
            return;
        }
        // A child window's realm runs this too, while the frame inserting
        // its iframe is still creating it: that window cannot resolve the
        // windows of other documents yet, so candidates of another displayed
        // document stay queued for the inserting window, not dropped.
        let foreign: Vec<Option<NodeId>> = {
            let dom = page.dom.borrow();
            batch.iter().map(|candidate| {
                let doc = match candidate {
                    pyro_dom::upgrade::UpgradeCandidate::Connected(el) => dom.node_document(*el).ok()?,
                    pyro_dom::upgrade::UpgradeCandidate::Created { document, .. } => *document,
                };
                (doc != page.document() && dom.is_in_active_document(doc)).then_some(doc)
            }).collect()
        };
        let mut resolved = FxHashMap::default();
        let (batch, deferred): (Vec<_>, Vec<_>) = batch.into_iter().zip(foreign).partition(|(_, doc)| {
            doc.is_none_or(|doc| *resolved.entry(doc).or_insert_with(|| document_page(page, doc).is_some()))
        });
        let batch: Vec<_> = batch.into_iter().map(|(candidate, _)| candidate).collect();
        let deferred: Vec<_> = deferred.into_iter().map(|(candidate, _)| candidate).collect();
        if !deferred.is_empty() {
            page.dom.borrow_mut().requeue_upgrade_reactions(deferred);
            if batch.is_empty() {
                return;
            }
        }
        // Capture strong definition and registry references before invoking
        // any author constructor. It may adopt a later sibling or remove
        // the document that provided that sibling's definition.
        let main = page.main_world_realm().map(|r| r.id()).unwrap_or(RealmId(0));
        let prepared: Vec<_> = batch.into_iter().map(|candidate| {
            let selected = match &candidate {
                pyro_dom::upgrade::UpgradeCandidate::Created { document, registry, name, .. } => {
                    registry_owner(page, *document, *registry, main)
                        .and_then(|(owner, key)| prepare_upgrade(owner, key, name.clone()))
                }
                pyro_dom::upgrade::UpgradeCandidate::Connected(_) => None,
            };
            (candidate, selected)
        }).collect();
        for (candidate, selected) in prepared {
            match candidate {
                pyro_dom::upgrade::UpgradeCandidate::Connected(el) => try_upgrade(page, el),
                pyro_dom::upgrade::UpgradeCandidate::Created { node, document, registry, .. } => {
                    if let Some(upgrade) = selected { upgrade.run(page, node); }
                    page.dom.borrow_mut().finish_created_upgrade(node, document, registry);
                }
            }
        }
    }
}

fn try_upgrade(page: &Rc<PageDom>, el: NodeId) {
    if page.dom.borrow().awaits_upgrade(el) {
        try_upgrade_element(page, el);
    }
}

/// Try the registry of the element, independently of connectedness (explicit
/// upgrade, clone, and initialize all include detached candidates).
fn try_upgrade_element(page: &Rc<PageDom>, el: NodeId) {
    let (local, is) = {
        let dom = page.dom.borrow();
        if dom.has_pending_created_upgrade(el) { return }
        let Ok(e) = dom.element(el) else { return };
        use pyro_dom::node::element_flags::{CUSTOM_DEFINED, CUSTOM_FAILED};
        if !e.is_html() || e.flags & (CUSTOM_DEFINED | CUSTOM_FAILED) != 0
            || (e.is_value.is_none() && !e.local_name().contains('-')) { return }
        (e.local_name().clone(), e.is_value.clone())
    };
    let main = page.main_world_realm().map(|r| r.id()).unwrap_or(RealmId(0));
    let Some((owner, key)) = registry_for_node_realm(page, el, main) else { return };
    let state = owner.custom_elements.borrow();
    let name = state.def_key(key, &local, is.as_ref()).or_else(|| {
        if key.scoped.is_some() { return None }
        state.defs.keys().filter(|(candidate, _)| candidate.scoped.is_none())
            .filter_map(|(candidate, _)| state.def_key(*candidate, &local, is.as_ref()).map(|name| (candidate.realm, name)))
            .min_by_key(|(realm, _)| *realm).map(|(_, name)| name)
    });
    drop(state);
    if let Some(selected) = name.and_then(|name| prepare_upgrade(owner, key, name)) { selected.run(page, el); }
}

struct PreparedUpgrade {
    realm: pyro_js::Realm,
    definition: UpgradeDefinition,
    // Scoped constructor/callback values are GC-traced by this wrapper.
    _registry: Option<Persistent>,
}

impl PreparedUpgrade {
    fn run(self, page: &Rc<PageDom>, el: NodeId) {
        let Ok(s) = self.realm.scope() else { return };
        if let Err(e) = upgrade(&s, page, el, self.definition) { s.report_exception(&e); }
    }
}

fn prepare_upgrade(owner: Rc<PageDom>, mut key: RegistryKey, name: LocalName) -> Option<PreparedUpgrade> {
    // Isolated worlds have distinct global registries. Honor the main world
    // first, then preserve a definition supplied by an isolated world.
    if key.scoped.is_none() {
        let state = owner.custom_elements.borrow();
        if !state.defs.contains_key(&(key, name.clone())) {
            let alternate = state.defs.keys().filter(|(candidate, n)| candidate.scoped.is_none() && n == &name)
                .map(|(candidate, _)| *candidate).min_by_key(|candidate| candidate.realm);
            key = alternate?;
        }
    }
    let realm = owner.realms.borrow().get(&key.realm)?.upgrade()?;
    let keep = if key.scoped.is_some() {
        let s = realm.scope().ok()?;
        Some(Persistent::new(&s, scoped_wrapper(&s, &owner, key)?.value()))
    } else { None };
    let definition = UpgradeDefinition::lookup(owner, key, name)?;
    Some(PreparedUpgrade { realm, definition, _registry: keep })
}

/// The bindings of the window that displays `doc`: `page`'s own, or those of
/// another document of its frame tree.
pub(crate) fn document_page(page: &Rc<PageDom>, doc: NodeId) -> Option<Rc<PageDom>> {
    if page.document() == doc {
        return Some(page.clone());
    }
    let realm = page.main_world_realm()?;
    let scope = realm.scope().ok()?;
    page.sibling_realms(&scope).iter().find_map(|sibling| {
        let s = sibling.scope().ok()?;
        page_of(&s).ok().filter(|p| Rc::ptr_eq(&p.dom, &page.dom) && p.document() == doc && !p.is_disposed())
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
    let lookup = {
        let dom = page.dom.borrow();
        let e = dom.element(el).js()?;
        e.is_html() && (e.is_value.is_some() || e.local_name().contains('-'))
    };
    if lookup {
        if let Some((owner, key)) = registry_for_node(s, page, el) {
            let definition = {
                let dom = page.dom.borrow();
                let e = dom.element(el).js()?;
                let registry = owner.custom_elements.borrow();
                registry.def_key(key, e.local_name(), e.is_value.as_ref()).map(|name| {
                    let fallback = (name == *e.local_name()).then(|| e.name.clone());
                    (name, fallback)
                })
            };
            if let Some((name, fallback)) = definition {
                let selected = UpgradeDefinition::lookup(owner.clone(), key, name)
                    .ok_or_else(|| JsError::Type("Missing element definition.".into()))?;
                if let Some(expected) = fallback {
                    let ctor = selected.definition.ctor.get(s);
                    CONSTRUCTIONS.with(|frames| frames.borrow_mut().push(ConstructionFrame {
                        constructor: Persistent::new(s, ctor), selected, upgrade: None,
                    }));
                    let _restore = ConstructionGuard;
                    match construct_fresh(s, page, doc, &expected, ctor, key.association()) {
                        Ok(result) => return Ok(result),
                        Err(error) => {
                            report_construction_error(&owner, key, &error);
                            let mut dom = page.dom.borrow_mut();
                            let failed = dom.create_element_raw(doc, expected).js()?;
                            dom.set_registry_association(failed, key.association()).js()?;
                            dom.set_custom_failed(failed).js()?;
                            drop(dom);
                            return Ok(crate::node::wrap_created_node(s, page, doc, failed)?.value());
                        }
                    }
                }
                if let Err(error) = upgrade(s, page, el, selected) {
                    report_construction_error(&owner, key, &error);
                }
                return Ok(wrap_node(s, page, el)?.value());
            }
        }
    }
    Ok(crate::node::wrap_created_node(s, page, doc, el)?.value())
}

fn report_construction_error(owner: &PageDom, key: RegistryKey, error: &JsError) {
    let realm = owner.realms.borrow().get(&key.realm).and_then(|r| r.upgrade());
    if let Some(realm) = realm {
        if let Ok(home) = realm.scope() { home.report_exception(error); }
    }
}

/// Synchronous autonomous creation calls the constructor with its registry
/// active. The HTMLElement constructor allocates; unlike an upgrade, there
/// is no existing element to consume. The namespace prefix is assigned only
/// to the validated result after author code returns.
fn construct_fresh<'s>(
    s: &Scope<'s>, page: &Rc<PageDom>, doc: NodeId, expected: &QualName, ctor: Value<'s>, association: RegistryAssociation,
) -> JsResult<Value<'s>> {
    let constructor = ctor.as_object().ok_or_else(|| JsError::Type("Not a constructor.".into()))?;
    let result = constructor.construct(&[])?;
    let element = s.unwrap::<crate::html::HtmlElement>(result.value())
        .ok_or_else(|| JsError::Type("The constructor did not return an HTMLElement.".into()))?;
    let invalid = || JsError::dom("NotSupportedError", "The constructor did not return a fresh element of the requested name and document.");
    if !Rc::ptr_eq(&page.dom, &element.page().dom) { return Err(invalid()) }
    let id = element.id;
    let mut dom = page.dom.borrow_mut();
    let e = dom.element(id).js()?;
    if e.name.ns != expected.ns || e.name.local != expected.local || !e.attrs.is_empty()
        || dom.first_child(id).js()?.is_some() || dom.parent(id).js()?.is_some() || dom.node_document(id).js()? != doc
    {
        return Err(invalid());
    }
    dom.set_element_prefix(id, expected.prefix.clone()).js()?;
    dom.set_is_value(id, None).js()?;
    dom.set_registry_association(id, association).js()?;
    Ok(result.value())
}

/// DOM "clone a node": a clone of a defined custom element made for the
/// page's document is upgraded at once, connected or not ("create an
/// element" with the synchronous custom elements flag unset enqueues an
/// upgrade reaction, run when the cloning call returns). Free while no
/// element is defined.
pub(crate) fn upgrade_clone(_s: &Scope<'_>, page: &Rc<PageDom>, root: NodeId) {
    if page.custom_elements.borrow().defs.is_empty() && !scoped_registries_exist() {
        return;
    }
    let Ok(candidates) = page.dom.borrow().shadow_including_nodes(root) else { return };
    for el in candidates {
        try_upgrade_element(page, el);
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
            (e.local_name().clone(), e.is_value.clone())
        };
        let Some(main) = page.main_world_realm() else { return ParserConstruct::NotDefined };
        let Some((owner, key)) = registry_for_node_realm(&page, el, main.id()) else { return ParserConstruct::NotDefined };
        let realm = owner.realms.borrow().get(&key.realm).and_then(|w| w.upgrade());
        let Some(realm) = realm.filter(|r| !r.is_disposed() || (key.scoped.is_some() && r.is_lingering())) else { return ParserConstruct::NotDefined };
        let Ok(s) = realm.scope() else { return ParserConstruct::NotDefined };
        let Some(name) = with_registry(&owner, |r| r.def_key(key, &name.0, name.1.as_ref())) else {
            return ParserConstruct::NotDefined;
        };
        let Some(definition) = UpgradeDefinition::lookup(owner, key, name) else { return ParserConstruct::NotDefined };
        // "Perform a microtask checkpoint" (no script runs here).
        crate::mutation_observer::checkpoint(&page);
        Self::counted(&page, || {
            let outcome = match construct_for_parser(&s, &page, el, definition) {
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
    type Data = RegistryObject;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| new_scoped(s, nt));
        def.method("define", 2, |s, registry, a| {
            let page = &registry.page;
            a.require(2, "CustomElementRegistry.define")?;
            let name = dom_string(a.get(0))?;
            let ctor = a.get(1);
            if !ctor.as_object().is_some_and(|c| c.is_constructor()) {
                return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': The provided value is not a constructor.".into()));
            }
            if !valid_name(&name) {
                return Err(JsError::dom("SyntaxError", format!("Failed to execute 'define' on 'CustomElementRegistry': \"{name}\" is not a valid custom element name")));
            }
            let key = (registry.key, LocalName::from(name.as_str()));
            // ElementDefinitionOptions `extends`: a customized built-in element of that HTML element.
            let mut local_name = key.1.clone();
            if let Some(options) = a.opt(2).filter(|v| !v.is_undefined() && !v.is_null()) {
                let Some(options) = options.as_object() else {
                    return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': The provided value is not of type 'ElementDefinitionOptions'.".into()));
                };
                let extends = options.get_atom("extends")?;
                if !extends.is_undefined() {
                    let extends = dom_string(extends)?;
                    if registry.key.scoped.is_some() {
                        return Err(JsError::dom("NotSupportedError", "A scoped registry cannot define customized built-in elements."));
                    }
                    if valid_name(&extends) {
                        return Err(JsError::dom("NotSupportedError", format!("Failed to execute 'define' on 'CustomElementRegistry': \"{extends}\" is a valid custom element name")));
                    }
                    if !crate::html::is_known_html_element(&extends) {
                        return Err(JsError::dom("NotSupportedError", format!("Failed to execute 'define' on 'CustomElementRegistry': \"{extends}\" is an HTMLUnknownElement")));
                    }
                    local_name = LocalName::from(extends.as_str());
                }
            }
            let exists = with_registry(page, |r| {
                r.defs.contains_key(&key) || r.defs.iter().any(|(k, d)| k.0 == key.0 && d.ctor.get(s).strict_equals(ctor))
            });
            if exists {
                return Err(JsError::dom("NotSupportedError", format!("Failed to execute 'define' on 'CustomElementRegistry': the name \"{name}\" has already been used with this registry")));
            }
            if !with_registry(page, |r| r.defining.insert(registry.key)) {
                return Err(JsError::dom("NotSupportedError", "Another definition is being processed in this registry."));
            }
            let result = (|| -> JsResult<(Definition, bool)> {
                let constructor = ctor.as_object().expect("constructor");
                // The lifecycle callbacks are those of the prototype at definition.
                let proto = constructor.get_atom("prototype")?;
                let Some(proto) = proto.as_object() else {
                    return Err(JsError::Type("Failed to execute 'define' on 'CustomElementRegistry': constructor prototype is not an object".into()));
                };
                let lifecycle = |name: &str| -> JsResult<Option<RegistryValue>> {
                    let value = proto.get(name)?;
                    if value.is_undefined() {
                        return Ok(None);
                    }
                    if !value.as_object().is_some_and(|f| f.is_function()) {
                        return Err(JsError::Type(format!("Failed to execute 'define' on 'CustomElementRegistry': {name} is not a function").into()));
                    }
                    Ok(Some(RegistryValue::Global(Persistent::new(s, value))))
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
                Ok((Definition {
                    ctor: RegistryValue::Global(Persistent::new(s, ctor)),
                    callbacks,
                    observed,
                    form_associated,
                    disable_internals,
                    disable_shadow,
                    local_name,
                }, disable_shadow))
            })();
            with_registry(page, |r| r.defining.remove(&registry.key));
            let (mut definition, disable_shadow) = result?;
            definition.trace_scoped(s, registry)?;
            let local_name = definition.local_name.clone();
            let observed = definition.observed.clone();
            with_registry(page, |r| r.defs.insert(key.clone(), Rc::new(definition)));
            let association = registry.key.association();
            let documents = registry_documents(registry);
            for arena in registry_arenas(registry, &documents) {
                // These are lookup filters, independent of whether this
                // arena has a document in the registry's scoped document set.
                let install_hooks = with_registry(&arena, |r| !std::mem::replace(&mut r.parser_hooks, true));
                let mut dom = arena.dom.borrow_mut();
                dom.enable_custom_upgrades();
                dom.note_registry_definition(arena.document(), association, &key.1, &local_name, observed.clone());
                if disable_shadow {
                    dom.disable_shadow_for_registry(arena.document(), association, &key.1, &local_name);
                }
                if install_hooks {
                    dom.set_parser_custom_elements(arena.document(), Some(Rc::new(ParserHooks { page: Rc::downgrade(&arena) })));
                }
            }
            // Snapshot all candidates before invoking constructors, in scoped
            // document-set order and shadow-including tree order within each.
            let mut existing = Vec::new();
            for (owner, doc) in documents {
                let candidates = owner.dom.borrow().upgrade_candidates_named(doc, &key.1);
                for el in candidates {
                    if registry_owns_node(s, registry, &owner, el) {
                        existing.push((owner.clone(), el));
                    }
                }
            }
            for (owner, el) in existing {
                try_upgrade_element(&owner, el);
            }
            let waiting = with_registry(page, |r| r.waiting.remove(&key));
            if let Some(d) = waiting {
                d.resolve(s, ctor)?;
            }
            Ok(s.undefined())
        });
        def.method("get", 1, |s, registry, a| {
            let page = &registry.page;
            let name = with_arg_str(&a, 0, "CustomElementRegistry.get", |name| Ok(LocalName::from(name)))?;
            Ok(with_registry(page, |r| r.ctor(s, registry.key, &name)).unwrap_or(s.undefined()))
        });
        def.method("getName", 1, |s, registry, a| {
            let page = &registry.page;
            a.require(1, "CustomElementRegistry.getName")?;
            let ctor = a.get(0);
            if !ctor.as_object().is_some_and(|o| o.is_function()) {
                return Err(JsError::Type("Expected a custom element constructor.".into()));
            }
            let name = with_registry(page, |r| {
                r.defs
                    .iter()
                    .find(|(k, d)| k.0 == registry.key && d.ctor.get(s).strict_equals(ctor))
                    .map(|(k, _)| k.1.clone())
            });
            Ok(name.map_or(s.null(), |n| s.string(&n)))
        });
        def.method("whenDefined", 1, |s, registry, a| {
            let page = &registry.page;
            a.require(1, "CustomElementRegistry.whenDefined")?;
            let name = dom_string(a.get(0))?;
            if !valid_name(&name) {
                let d = s.deferred()?;
                d.reject(
                    s,
                    JsError::dom("SyntaxError", format!("\"{name}\" is not a valid custom element name")).to_value(s),
                )?;
                return Ok(d.promise.get(s));
            }
            let key = (registry.key, LocalName::from(name.as_str()));
            if let Some(promise) = with_registry(page, |r| r.waiting.get(&key).map(|d| d.promise(s))) {
                return Ok(promise);
            }
            let d = s.deferred()?;
            let existing = with_registry(page, |r| r.ctor(s, key.0, &key.1));
            let promise = d.promise.get(s);
            match existing {
                Some(ctor) => d.resolve(s, ctor)?,
                None => {
                    let pending = PendingDefinition::new(s, registry, d)?;
                    with_registry(page, |r| r.waiting.insert(key, pending));
                }
            }
            Ok(promise)
        });
        def.method("upgrade", 1, |s, registry, a| {
            a.require(1, "CustomElementRegistry.upgrade")?;
            let (page, root) = registry_root(s, a.get(0))?;
            let nodes = page.dom.borrow().shadow_including_nodes(root).js()?;
            for el in nodes {
                if page.dom.borrow().element(el).is_ok() && registry_owns_node(s, registry, &page, el) {
                    try_upgrade_element(&page, el);
                }
            }
            Ok(s.undefined())
        });
        def.method("initialize", 1, |s, registry, a| {
            a.require(1, "CustomElementRegistry.initialize")?;
            let (page, root) = registry_root(s, a.get(0))?;
            let doc = page.dom.borrow().node_document(root).js()?;
            let is_document = page.dom.borrow().node_type(root).js()? == pyro_dom::NodeType::Document;
            if registry.key.scoped.is_none() && (is_document || !registry_owns_node(s, registry, &page, doc)) {
                return Err(JsError::dom("NotSupportedError", "This global registry cannot initialize the given root."));
            }
            let association = registry.key.association();
            let is_shadow = page.dom.borrow().is_shadow_root(root).js()?;
            let nodes: Vec<NodeId> = {
                let dom = page.dom.borrow();
                dom.descendants(root).js()?.filter(|&n| dom.element(n).is_ok()).collect()
            };
            // Determine null entries before changing a Document's registry:
            // existing elements with null associations must be updated too.
            let initialize_root = (is_document || is_shadow) && registry_for_node(s, &page, root).is_none();
            let mut upgrades = Vec::new();
            for el in nodes {
                if registry_for_node(s, &page, el).is_none() {
                    page.dom.borrow_mut().set_registry_association(el, association).js()?;
                    note_registry_document(registry, &page, doc);
                    upgrades.push(el);
                } else if registry_owns_node(s, registry, &page, el) {
                    upgrades.push(el);
                }
            }
            if initialize_root {
                page.dom.borrow_mut().set_registry_association(root, association).js()?;
                pin_registry(registry, &page, doc);
            }
            for el in upgrades {
                try_upgrade_element(&page, el);
            }
            Ok(s.undefined())
        });
    }
}

fn registry_root(s: &Scope<'_>, value: Value<'_>) -> JsResult<(Rc<PageDom>, NodeId)> {
    let node = s.unwrap::<crate::node::Node>(value).ok_or_else(|| JsError::Type("Expected a Node.".into()))?;
    Ok((node.page().clone(), node.id))
}

fn registry_owns_node(s: &Scope<'_>, registry: &RegistryObject, page: &Rc<PageDom>, node: NodeId) -> bool {
    registry_for_node(s, page, node).is_some_and(|(owner, key)| key == registry.key && Rc::ptr_eq(&owner, &registry.page))
}

/// `window.customElements` (one registry object per realm).
pub fn registry<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let key = pyro_js::IdentityKey(page.id() | (1 << 58), 0);
    crate::window::same_object(s, page, key, || s.wrap::<CustomElementRegistry>(RegistryObject { page: page.clone(), key: RegistryKey::global(s.realm_id()) }, Some(key)))
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<CustomElementRegistry>();
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    RUNNING.with(|r| !r.borrow().is_empty()).then_some("custom::RUNNING")
}
