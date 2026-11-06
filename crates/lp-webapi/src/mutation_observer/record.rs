use super::*;
use crate::collections::{static_node_list, NodeHold};
use lp_dom::mutation::{MutationRecord as NativeRecord, RecordKind};

pub struct RecordData {
    page: Rc<PageDom>,
    record: NativeRecord,
    // Retain every referenced node before any JS allocation/callback can GC.
    _holds: Vec<NodeHold>,
    identity: IdentityKey,
    realm: WeakRealm,
}
pub struct MutationRecord;
impl JsClass for MutationRecord {
    const NAME: &'static str = "MutationRecord";
    type Data = RecordData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("type", |s, d| {
            Ok(s.string(match d.record.kind {
                RecordKind::Attributes => "attributes",
                RecordKind::CharacterData => "characterData",
                RecordKind::ChildList => "childList",
            }))
        });
        def.getter("target", |s, d| Ok(crate::node::wrap_node(s, &d.page, d.record.target)?.value()));
        def.getter("previousSibling", |s, d| crate::util::opt_node(s, &d.page, d.record.previous_sibling));
        def.getter("nextSibling", |s, d| crate::util::opt_node(s, &d.page, d.record.next_sibling));
        def.getter("attributeName", |s, d| Ok(d.record.attribute_name.as_ref().map_or(s.null(), |v| s.string(v))));
        def.getter("attributeNamespace", |s, d| {
            Ok(d.record.attribute_namespace.as_ref().filter(|v| !v.is_empty()).map_or(s.null(), |v| s.string(v)))
        });
        def.getter("oldValue", |s, d| Ok(d.record.old_value.as_ref().map_or(s.null(), |v| s.string(v))));
        def.getter("addedNodes", |s, d| list(s, d, "added"));
        def.getter("removedNodes", |s, d| list(s, d, "removed"));
    }
}
/// `addedNodes` / `removedNodes`: built on first access (most observers
/// never read them) and kept on the record, so every access returns the
/// same NodeList.
fn list<'s>(s: &Scope<'s>, data: &RecordData, name: &'static str) -> JsResult<Value<'s>> {
    let realm = data.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
    let owner = realm.scope()?;
    let object = owner.lookup_identity(data.identity).ok_or(JsError::ContextDestroyed)?;
    // SAFETY: realm and caller belong to the same VM; getter receiver roots it.
    let object = unsafe { Object::from_raw(s.ctx(), object.as_raw()) };
    if let Some(list) = s.get_hidden(object, name)? {
        return Ok(list);
    }
    let ids = if name == "added" { &data.record.added } else { &data.record.removed };
    let list = static_node_list(&owner, &data.page, ids.clone())?;
    // SAFETY: as above.
    let list = unsafe { Value::from_raw(s.ctx(), list.value().as_raw()) };
    s.set_hidden(object, name, list)?;
    Ok(list)
}

pub(super) struct HeldRecord {
    page: Rc<PageDom>,
    record: NativeRecord,
    holds: Vec<NodeHold>,
}
pub(super) fn take(page: &Rc<PageDom>, id: ObserverId) -> JsResult<Vec<HeldRecord>> {
    let records = page.dom.borrow_mut().take_records(id);
    records
        .into_iter()
        .map(|record| {
            let ids = std::iter::once(record.target)
                .chain(record.added.iter().copied())
                .chain(record.removed.iter().copied())
                .chain(record.previous_sibling)
                .chain(record.next_sibling);
            let holds = ids.map(|id| NodeHold::new(page, id)).collect::<JsResult<Vec<_>>>()?;
            Ok(HeldRecord { page: page.clone(), record, holds })
        })
        .collect()
}
pub(super) fn array<'s>(s: &Scope<'s>, records: Vec<HeldRecord>) -> JsResult<Value<'s>> {
    let mut values = lp_js::RootedVec::new();
    for held in records {
        let page = held.page.clone();
        let identity = page.event_identity(page.next_event_id());
        let object = s.wrap::<MutationRecord>(
            RecordData {
                page: held.page,
                record: held.record,
                _holds: held.holds,
                identity,
                realm: s.realm().downgrade(),
            },
            Some(identity),
        )?;
        values.push(s, object.value());
    }
    Ok(s.array(&values.iter(s).collect::<Vec<_>>())?.value())
}
