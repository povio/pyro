//! IndexedDB discovery only. The context storage partition has an independent
//! database catalog per origin. Creation, restoration, deletion and transactions
//! are unsupported and fail explicitly; no supplied database state is discarded.
use crate::{page::PageDom, util::arg_string};
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, RootedVec, Scope, Value};
use rustc_hash::FxHashMap;
use std::{
    cell::RefCell,
    rc::{Rc, Weak},
};

#[derive(Clone, Default)]
pub(crate) struct Namespace(Rc<RefCell<FxHashMap<String, Vec<DatabaseInfo>>>>);
#[derive(Clone)]
struct DatabaseInfo {
    name: String,
    version: u64,
}
impl Namespace {
    fn databases(&self, origin: &str) -> Vec<DatabaseInfo> {
        self.0.borrow().get(origin).cloned().unwrap_or_default()
    }
}

pub struct FactoryData {
    page: Weak<PageDom>,
    origin: Option<String>,
    namespace: Namespace,
}
impl FactoryData {
    fn origin(&self) -> JsResult<&str> {
        if self.page.upgrade().is_none_or(|page| page.is_disposed()) {
            return Err(JsError::ContextDestroyed);
        }
        self.origin
            .as_deref()
            .ok_or_else(|| JsError::dom("SecurityError", "IndexedDB is unavailable for an opaque origin."))
    }
}
pub struct IdbFactory;
impl JsClass for IdbFactory {
    const NAME: &'static str = "IDBFactory";
    type Data = FactoryData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("databases", 0, |s, data, _| {
            let promise = s.deferred()?;
            match data.origin() {
                Ok(origin) => {
                    let mut rows = RootedVec::new();
                    // Snapshot native metadata before invoking JS setters or
                    // resolving promises; no namespace borrow crosses JS entry.
                    for info in data.namespace.databases(origin) {
                        let row = s.object();
                        row.set("name", s.string(&info.name))?;
                        row.set("version", s.number(info.version as f64))?;
                        rows.push(s, row.value());
                    }
                    promise.resolve(s, s.array(&rows.iter(s).collect::<Vec<_>>())?.value())?;
                }
                Err(error) => promise.reject(s, error.to_value(s))?,
            }
            Ok(promise.promise.get(s))
        });
        def.method("open", 1, |_s, data, args| {
            let _name = arg_string(&args, 0, "IDBFactory.open")?;
            data.origin()?;
            Err(JsError::dom("NotSupportedError", "IndexedDB database creation and restoration are not implemented."))
        });
        def.method("deleteDatabase", 1, |_s, data, args| {
            let _name = arg_string(&args, 0, "IDBFactory.deleteDatabase")?;
            data.origin()?;
            Err(JsError::dom("NotSupportedError", "IndexedDB database deletion is not implemented."))
        });
        def.method("cmp", 2, |_s, _data, args| {
            args.require(2, "IDBFactory.cmp")?;
            Err(JsError::dom("NotSupportedError", "IndexedDB key comparison is not implemented."))
        });
    }
}

pub(crate) fn window_factory<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    let key = IdentityKey(page.id() | (1 << 57), 2);
    crate::window::same_object(s, page, key, || {
        let data = FactoryData {
            page: Rc::downgrade(page),
            origin: crate::storage::storage_origin(s, page).ok(),
            namespace: page.local_storage.borrow().indexed_db(),
        };
        s.wrap::<IdbFactory>(data, Some(key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalog_snapshots_are_origin_and_context_scoped() {
        let context = Namespace::default();
        let shared = context.clone();
        let isolated = Namespace::default();
        // Test-only metadata verifies discovery reads the partition's catalog,
        // rather than returning a constant empty list. Production has no
        // database creator/importer until those operations are implemented.
        context.0.borrow_mut().insert("http://one.test".into(), vec![DatabaseInfo { name: "one".into(), version: 3 }]);
        let mut snapshot = shared.databases("http://one.test");
        assert_eq!((&*snapshot[0].name, snapshot[0].version), ("one", 3));
        snapshot.clear();
        assert_eq!(shared.databases("http://one.test").len(), 1);
        assert!(shared.databases("http://two.test").is_empty());
        assert!(isolated.databases("http://one.test").is_empty());
    }
}
