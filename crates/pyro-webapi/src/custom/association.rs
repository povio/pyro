//! Registry identity survives movement of an associated element between documents.

use super::*;
use pyro_js::IdentityKey;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) struct RegistryKey {
    pub(super) realm: RealmId,
    pub(super) scoped: Option<u64>,
}

impl RegistryKey {
    pub(super) fn global(realm: RealmId) -> Self {
        Self { realm, scoped: None }
    }

    pub(super) fn identity(self, page: &PageDom) -> IdentityKey {
        IdentityKey(page.id() | (1 << 58), self.scoped.unwrap_or(0))
    }

    pub(super) fn association(self) -> RegistryAssociation {
        self.scoped.map_or(RegistryAssociation::Default, RegistryAssociation::Scoped)
    }
}

pub struct RegistryObject {
    pub(super) page: Rc<PageDom>,
    pub(super) key: RegistryKey,
}

pub(super) struct ScopedEntry {
    pub(super) realm: RealmId,
    documents: Vec<(Weak<PageDom>, NodeId)>,
}

// IDs can cross documents when nodes are adopted. The index itself owns no page.
thread_local! {
    static OWNERS: RefCell<FxHashMap<u64, Weak<PageDom>>> = RefCell::default();
}

pub(super) fn scoped_owner(id: u64) -> Option<Rc<PageDom>> {
    OWNERS.with(|owners| owners.borrow().get(&id)?.upgrade())
}

pub(super) fn forget_scoped(id: u64) {
    OWNERS.with(|owners| owners.borrow_mut().remove(&id));
}

pub(super) fn new_scoped<'s>(s: &Scope<'s>, new_target: Object<'_>) -> JsResult<Object<'s>> {
    static NEXT_ID: AtomicU64 = AtomicU64::new(1);
    let page = page_of(s)?;
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    let key = RegistryKey { realm: s.realm_id(), scoped: Some(id) };
    let wrapper = s.wrap_new::<CustomElementRegistry>(
        RegistryObject { page: page.clone(), key },
        new_target,
        Some(key.identity(&page)),
    )?;
    page.custom_elements.borrow_mut().scoped.insert(id, ScopedEntry { realm: key.realm, documents: Vec::new() });
    OWNERS.with(|owners| owners.borrow_mut().insert(id, Rc::downgrade(&page)));
    Ok(wrapper)
}

impl Drop for RegistryObject {
    fn drop(&mut self) {
        let Some(id) = self.key.scoped else { return };
        forget_scoped(id);
        if let Ok(mut state) = self.page.custom_elements.try_borrow_mut() {
            if let Some(entry) = state.scoped.remove(&id) {
                for arena in
                    std::iter::once(self.page.clone()).chain(entry.documents.iter().filter_map(|(w, _)| w.upgrade()))
                {
                    if let Ok(mut dom) = arena.dom.try_borrow_mut() {
                        dom.forget_scoped_registry(id);
                    }
                }
            }
            state.defs.retain(|(key, _), _| *key != self.key);
            state.waiting.retain(|(key, _), _| *key != self.key);
        }
    }
}

pub(super) fn pin_registry(obj: &RegistryObject, page: &Rc<PageDom>, doc: NodeId) {
    let Some(id) = obj.key.scoped else { return };
    let first_arena = {
        let mut state = obj.page.custom_elements.borrow_mut();
        let Some(entry) = state.scoped.get_mut(&id) else { return };
        let first_arena = !Rc::ptr_eq(&obj.page.dom, &page.dom)
            && !entry.documents.iter().filter_map(|(w, _)| w.upgrade()).any(|p| Rc::ptr_eq(&p.dom, &page.dom));
        if !entry.documents.iter().any(|(owner, node)| *node == doc && owner.ptr_eq(&Rc::downgrade(page))) {
            entry.documents.push((Rc::downgrade(page), doc));
        }
        first_arena
    };
    let pin_owner = document_page(page, doc).unwrap_or_else(|| page.clone());
    install_pin(&pin_owner, id, &obj.page, obj.key);
    if first_arena {
        let mut dom = page.dom.borrow_mut();
        dom.enable_custom_upgrades();
        for ((key, name), definition) in &obj.page.custom_elements.borrow().defs {
            if *key != obj.key {
                continue;
            }
            dom.note_registry_definition(doc, key.association(), name, &definition.local_name, definition.observed.clone());
            if definition.disable_shadow {
                dom.disable_shadow_for_registry(doc, key.association(), name, &definition.local_name);
            }
        }
        dom.set_parser_custom_elements(doc, Some(Rc::new(ParserHooks { page: Rc::downgrade(page) })));
    }
}

pub(super) fn note_registry_document(obj: &RegistryObject, page: &Rc<PageDom>, doc: NodeId) {
    pin_registry(obj, page, doc);
    if let Some(id) = obj.key.scoped {
        let _ = page.dom.borrow_mut().note_scoped_registry_document(id, doc);
    }
}

/// Every arena that has associated a node with this registry needs the DOM's
/// definition filters, even if that node has never been connected.
pub(super) fn registry_arenas(obj: &RegistryObject, documents: &[(Rc<PageDom>, NodeId)]) -> Vec<Rc<PageDom>> {
    let Some(id) = obj.key.scoped else { return vec![obj.page.clone()] };
    let state = obj.page.custom_elements.borrow();
    let observed =
        state.scoped.get(&id).into_iter().flat_map(|entry| &entry.documents).filter_map(|(page, _)| page.upgrade());
    let mut seen = FxHashSet::default();
    std::iter::once(obj.page.clone())
        .chain(observed)
        .chain(documents.iter().map(|(page, _)| page.clone()))
        .filter(|page| seen.insert(Rc::as_ptr(&page.dom)))
        .collect()
}

pub(super) fn registry_documents(obj: &RegistryObject) -> Vec<(Rc<PageDom>, NodeId)> {
    let Some(id) = obj.key.scoped else { return vec![(obj.page.clone(), obj.page.document())] };
    let state = obj.page.custom_elements.borrow();
    let Some(entry) = state.scoped.get(&id) else { return Vec::new() };
    let mut arenas: Vec<Rc<PageDom>> = Vec::new();
    let mut documents = Vec::new();
    // DOM insertion, including adoption and declarative shadow trees, records
    // the actual time each document first became associated with the registry.
    let tree = obj.page.hooks().map(|h| h.frame_tree()).unwrap_or_default();
    for page in
        std::iter::once(obj.page.clone()).chain(tree).chain(entry.documents.iter().filter_map(|(w, _)| w.upgrade()))
    {
        if arenas.iter().any(|a| Rc::ptr_eq(&a.dom, &page.dom)) {
            continue;
        }
        for (order, doc) in page.dom.borrow().scoped_registry_documents(id) {
            documents.push((order, page.clone(), doc));
        }
        arenas.push(page);
    }
    documents.sort_by_key(|(order, _, _)| *order);
    documents
        .into_iter()
        .map(|(_, page, doc)| {
            let owner = document_page(&page, doc).unwrap_or(page);
            (owner, doc)
        })
        .collect()
}

pub(super) fn registry_for_node_realm(
    page: &Rc<PageDom>,
    node: NodeId,
    realm: RealmId,
) -> Option<(Rc<PageDom>, RegistryKey)> {
    let dom = page.dom.borrow();
    let mut association = dom.registry_association(node).ok()?;
    let doc = dom.node_document(node).ok()?;
    if association == RegistryAssociation::Default && doc != node {
        association = dom.registry_association(doc).ok()?;
    }
    drop(dom);
    match association {
        RegistryAssociation::Null => None,
        RegistryAssociation::Default => {
            let owner = document_page(page, doc)?;
            let realm = if owner.realms.borrow().contains_key(&realm) { realm } else { owner.main_world_realm()?.id() };
            Some((owner, RegistryKey::global(realm)))
        }
        RegistryAssociation::Scoped(id) => {
            let owner = scoped_owner(id)?;
            let realm = owner.custom_elements.borrow().scoped.get(&id)?.realm;
            Some((owner, RegistryKey { scoped: Some(id), realm }))
        }
    }
}

pub(super) fn registry_for_node(s: &Scope<'_>, page: &Rc<PageDom>, node: NodeId) -> Option<(Rc<PageDom>, RegistryKey)> {
    registry_for_node_realm(page, node, s.realm_id())
}

pub(crate) fn node_registry<'s>(s: &Scope<'s>, page: &Rc<PageDom>, node: NodeId) -> JsResult<Value<'s>> {
    let Some((owner, key)) = registry_for_node(s, page, node) else { return Ok(s.null()) };
    if key.scoped.is_some() {
        return Ok(scoped_wrapper(s, &owner, key).map_or(s.null(), |w| w.value()));
    }
    if s.realm_id() == key.realm {
        return registry(s, &owner);
    }
    let realm = owner.realms.borrow().get(&key.realm).and_then(|w| w.upgrade()).ok_or(JsError::ContextDestroyed)?;
    let home = realm.scope()?;
    Ok(s.rehome(registry(&home, &owner)?))
}

pub(crate) fn registry_argument(
    s: &Scope<'_>,
    page: &Rc<PageDom>,
    doc: NodeId,
    value: Value<'_>,
) -> JsResult<RegistryAssociation> {
    if value.is_null() {
        return Ok(RegistryAssociation::Null);
    }
    let obj = s
        .unwrap::<CustomElementRegistry>(value)
        .ok_or_else(|| JsError::Type("Expected a CustomElementRegistry.".into()))?;
    if obj.key.scoped.is_none() && (obj.page.document() != doc || !Rc::ptr_eq(&obj.page.dom, &page.dom)) {
        return Err(JsError::dom("NotSupportedError", "The global registry belongs to another document."));
    }
    pin_registry(&obj, page, doc);
    Ok(obj.key.association())
}

pub(super) fn scoped_registries_exist() -> bool {
    OWNERS.with(|owners| !owners.borrow().is_empty())
}

/// Resolve the original wrapper without creating a same-realm substitute.
pub(super) fn scoped_wrapper<'s>(s: &Scope<'s>, owner: &PageDom, key: RegistryKey) -> Option<Object<'s>> {
    let identity = key.identity(owner);
    if s.realm_id() == key.realm {
        return s.lookup_identity(identity);
    }
    let realm = owner.realms.borrow().get(&key.realm)?.upgrade()?;
    let home = realm.scope().ok()?;
    Some(s.rehome_object(home.lookup_identity(identity)?))
}

// An active document must keep the registries of its native nodes alive.
// Trace those edges from its JS Document wrapper: a registry whose callbacks
// refer to that document forms a normal collectible JavaScript cycle.
fn pin_holder<'s>(s: &Scope<'s>, page: &PageDom, create: bool) -> JsResult<Option<Object<'s>>> {
    let document = wrap_node(s, page, page.document())?;
    if let Some(holder) = s.get_hidden(document, "customRegistryPins")?.and_then(|v| v.as_object()) {
        return Ok(Some(holder));
    }
    if !create {
        return Ok(None);
    }
    let holder = s.object();
    holder.set_prototype(s.null());
    s.set_hidden(document, "customRegistryPins", holder.value())?;
    Ok(Some(holder))
}

fn install_pin(page: &Rc<PageDom>, id: u64, owner: &Rc<PageDom>, key: RegistryKey) {
    if page.custom_elements.borrow().pinned.contains(&id) {
        return;
    }
    let Some(realm) = page.main_world_realm() else { return };
    let Ok(s) = realm.scope() else { return };
    let Some(wrapper) = scoped_wrapper(&s, owner, key) else { return };
    if let Ok(Some(holder)) = pin_holder(&s, page, true) {
        if holder.set(&id.to_string(), wrapper.value()).is_ok() {
            page.custom_elements.borrow_mut().pinned.insert(id);
        }
    }
}

pub(super) fn prune_unassociated(page: &Rc<PageDom>) {
    if !scoped_registries_exist() && page.custom_elements.borrow().pinned.is_empty() {
        return;
    }
    let live = page.dom.borrow().live_scoped_registries();
    for &id in &live {
        if page.custom_elements.borrow().pinned.contains(&id) {
            continue;
        }
        let Some(owner) = scoped_owner(id) else { continue };
        let realm = owner.custom_elements.borrow().scoped.get(&id).map(|entry| entry.realm);
        if let Some(realm) = realm {
            install_pin(page, id, &owner, RegistryKey { realm, scoped: Some(id) });
        }
    }
    let stale: Vec<_> = page.custom_elements.borrow().pinned.difference(&live).copied().collect();
    if stale.is_empty() {
        return;
    }
    if let Some(realm) = page.main_world_realm() {
        if let Ok(s) = realm.scope() {
            if let Ok(Some(holder)) = pin_holder(&s, page, false) {
                for id in stale {
                    if holder.delete(&id.to_string()).is_ok() {
                        page.custom_elements.borrow_mut().pinned.remove(&id);
                    }
                }
            }
        }
    }
}
