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

    fn identity(self, page: &PageDom) -> IdentityKey {
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
    wrapper: Option<Persistent>,
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
        RegistryObject { page: page.clone(), key }, new_target, Some(key.identity(&page)),
    )?;
    page.custom_elements.borrow_mut().scoped.insert(id, ScopedEntry {
        realm: key.realm, documents: Vec::new(), wrapper: None,
    });
    OWNERS.with(|owners| owners.borrow_mut().insert(id, Rc::downgrade(&page)));
    Ok(wrapper)
}

impl Drop for RegistryObject {
    fn drop(&mut self) {
        let Some(id) = self.key.scoped else { return };
        forget_scoped(id);
        if let Ok(mut state) = self.page.custom_elements.try_borrow_mut() {
            state.scoped.remove(&id);
            state.defs.retain(|(key, _), _| *key != self.key);
            state.waiting.retain(|(key, _), _| *key != self.key);
        }
    }
}

pub(super) fn note_registry_document(obj: &RegistryObject, page: &Rc<PageDom>, doc: NodeId) {
    let Some(id) = obj.key.scoped else { return };
    let mut state = obj.page.custom_elements.borrow_mut();
    let Some(entry) = state.scoped.get_mut(&id) else { return };
    if !entry.documents.iter().any(|(owner, node)| *node == doc && owner.ptr_eq(&Rc::downgrade(page))) {
        entry.documents.push((Rc::downgrade(page), doc));
    }
    if entry.wrapper.is_none() {
        let realm = obj.page.realms.borrow().get(&obj.key.realm).and_then(|w| w.upgrade());
        if let Some(realm) = realm {
            if let Ok(s) = realm.scope() {
                if let Some(wrapper) = s.lookup_identity(obj.key.identity(&obj.page)) {
                    entry.wrapper = Some(Persistent::new(&s, wrapper.value()));
                }
            }
        }
    }
}

pub(super) fn registry_documents(obj: &RegistryObject) -> Vec<(Rc<PageDom>, NodeId)> {
    let Some(id) = obj.key.scoped else { return vec![(obj.page.clone(), obj.page.document())] };
    let state = obj.page.custom_elements.borrow();
    state.scoped.get(&id).map(|e| e.documents.iter().filter_map(|(w, doc)| Some((w.upgrade()?, *doc))).collect()).unwrap_or_default()
}

pub(super) fn registry_for_node_realm(page: &Rc<PageDom>, node: NodeId, realm: RealmId) -> Option<(Rc<PageDom>, RegistryKey)> {
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
    if let Some(id) = key.scoped {
        let state = owner.custom_elements.borrow();
        return Ok(state.scoped.get(&id).and_then(|e| e.wrapper.as_ref()).map_or(s.null(), |p| p.get(s)));
    }
    if s.realm_id() == key.realm { return registry(s, &owner) }
    let realm = owner.realms.borrow().get(&key.realm).and_then(|w| w.upgrade()).ok_or(JsError::ContextDestroyed)?;
    let home = realm.scope()?;
    Ok(s.rehome(registry(&home, &owner)?))
}

pub(crate) fn registry_argument(s: &Scope<'_>, page: &Rc<PageDom>, doc: NodeId, value: Value<'_>) -> JsResult<RegistryAssociation> {
    if value.is_null() { return Ok(RegistryAssociation::Null) }
    let obj = s.unwrap::<CustomElementRegistry>(value).ok_or_else(|| JsError::Type("Expected a CustomElementRegistry.".into()))?;
    if obj.key.scoped.is_none() && (obj.page.document() != doc || !Rc::ptr_eq(&obj.page.dom, &page.dom)) {
        return Err(JsError::dom("NotSupportedError", "The global registry belongs to another document."));
    }
    note_registry_document(&obj, page, doc);
    Ok(obj.key.association())
}


pub(super) fn scoped_registries_exist() -> bool {
    OWNERS.with(|owners| !owners.borrow().is_empty())
}

pub(super) fn prune_unassociated(page: &PageDom) {
    let mut state = page.custom_elements.borrow_mut();
    if state.scoped.is_empty() { return }
    let mut released = Vec::new();
    for (&id, entry) in &mut state.scoped {
        if entry.wrapper.is_none() { continue }
        entry.documents.retain(|(owner, _)| owner.strong_count() != 0);
        let still_associated = entry.documents.iter().filter_map(|(owner, _)| owner.upgrade())
            .any(|owner| owner.dom.try_borrow().map_or(true, |dom| dom.has_scoped_registry(id)));
        if !still_associated {
            released.push(entry.wrapper.take());
        }
    }
    drop(state);
    drop(released);
}
