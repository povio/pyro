//! Storage Buckets interfaces on secure window and worker globals. Each
//! opened bucket wrapper retains its generation, even after its name is reused.
pub(crate) mod catalog;

use std::rc::{Rc, Weak};
use std::sync::Arc;

use pyro_js::{ClassDef, Engine, JsClass, JsError, JsResult, NoParent, Object, RootedVec, Scope, Value};

use crate::page::PageDom;
use crate::util::arg_string;
use catalog::{Bucket, Error, Options, StorageKey};

pub(crate) fn storage_key(s: &Scope<'_>, page: &PageDom) -> Option<StorageKey> {
    Some(StorageKey {
        origin: crate::storage::tuple_origin(s, page)?,
        top_level_site: page.cookie_partition_key(),
        cross_site_ancestor: page.cookie_site_context() == pyro_net::cookies::SiteContext::CrossSite,
    })
}

fn closed() -> JsError {
    JsError::Type("The storage bucket's environment is no longer active.".into())
}

fn owner(page: &Weak<PageDom>) -> JsResult<Rc<PageDom>> {
    page.upgrade().filter(|p| p.is_fully_active()).ok_or_else(closed)
}

fn api_error(error: Error) -> JsError {
    match error {
        Error::InvalidName => JsError::Type("Invalid storage bucket name.".into()),
        Error::InvalidQuota => JsError::Type("Invalid storage bucket quota.".into()),
        Error::InvalidExpiration => JsError::Type("Invalid storage bucket expiration.".into()),
        Error::Removed => JsError::dom("UnknownError", "The storage bucket no longer exists."),
        Error::QuotaExceeded => JsError::dom("QuotaExceededError", "The storage bucket quota was exceeded."),
    }
}

/// Queue bucket mutations and queries after argument conversion; no native
/// catalog or backend lock is held while the resulting promise settles.
fn task<'s>(
    s: &Scope<'s>,
    page: &Weak<PageDom>,
    operation: impl for<'a> FnOnce(&Scope<'a>, &Rc<PageDom>) -> JsResult<Value<'a>> + 'static,
) -> JsResult<Value<'s>> {
    let active = owner(page)?;
    let page = page.clone();
    let realm = s.realm().downgrade();
    let deferred = s.deferred()?;
    let promise = deferred.promise.get(s);
    active.post_task(Box::new(move || {
        let Some(realm) = realm.upgrade() else { return };
        let Ok(scope) = realm.scope() else { return };
        let settled = match owner(&page).and_then(|p| operation(&scope, &p)) {
            Ok(result) => deferred.resolve(&scope, result),
            Err(error) => deferred.reject(&scope, error.to_value(&scope)),
        };
        if let Err(error) = settled {
            scope.report_exception(&error);
        }
    }));
    Ok(promise)
}

fn finite_number(s: &Scope<'_>, value: Value<'_>) -> JsResult<f64> {
    // JS's unary plus performs Web IDL ToNumber, including TypeError when
    // ToPrimitive produces a BigInt. The JSC C API number conversion accepts it.
    let holder = crate::worker_scope::module_holder(s, &crate::target::page_of(s)?)?;
    const SLOT: &str = "lpBucketToNumber";
    let convert = match s.get_hidden(holder, SLOT)?.and_then(|v| v.as_object()) {
        Some(f) => f,
        None => {
            let f = s.eval("(value => +value)", None, 1)?.as_object().expect("numeric conversion function");
            s.set_hidden(holder, SLOT, f.value())?;
            f
        }
    };
    let n = convert.call(None, &[value])?.to_number()?;
    if n.is_finite() { Ok(n) } else { Err(JsError::Type("Expected a finite number.".into())) }
}

fn options(s: &Scope<'_>, dictionary: Value<'_>) -> JsResult<Options> {
    let mut options = Options { expires: None, persisted: false, quota: None };
    if dictionary.is_nullish() { return Ok(options); }
    let object = dictionary.as_object().ok_or_else(|| JsError::Type("Expected a StorageBucketOptions dictionary.".into()))?;
    // Web IDL dictionary members are read in lexicographic order.
    let expires = object.get_atom("expires")?;
    if !expires.is_undefined() {
        options.expires = Some(finite_number(s, expires)?);
    }
    options.persisted = object.get_atom("persisted")?.to_bool();
    let quota = object.get_atom("quota")?;
    if !quota.is_undefined() {
        let n = finite_number(s, quota)?.trunc();
        if !(1.0..=9_007_199_254_740_991.0).contains(&n) {
            return Err(api_error(Error::InvalidQuota));
        }
        options.quota = Some(n as u64);
    }
    Ok(options)
}

struct ManagerData {
    page: Weak<PageDom>,
    key: Option<StorageKey>,
}

impl ManagerData {
    fn key(&self) -> JsResult<StorageKey> {
        owner(&self.page)?;
        self.key.clone().ok_or_else(|| JsError::dom("SecurityError", "Storage buckets are unavailable on an opaque origin."))
    }
}

struct StorageBucketManager;
impl JsClass for StorageBucketManager {
    const NAME: &'static str = "StorageBucketManager";
    type Data = ManagerData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.promise_method("open", 1, |s, m, a| {
            let name = arg_string(&a, 0, "StorageBucketManager.open")?;
            let options = options(s, a.get(1))?;
            let key = m.key()?;
            task(s, &m.page, move |s, page| {
                let catalog = page.local_storage.borrow().buckets();
                let bucket = catalog.open(&key, &name, options).map_err(api_error)?;
                Ok(s.wrap::<StorageBucket>(BucketData { page: Rc::downgrade(page), bucket }, None)?.value())
            })
        });
        def.promise_method("keys", 0, |s, m, _| {
            let key = m.key()?;
            task(s, &m.page, move |s, page| {
                let catalog = page.local_storage.borrow().buckets();
                let names = catalog.keys(&key);
                let mut result = RootedVec::new();
                for name in names { result.push(s, s.string(&name)); }
                Ok(s.array(&result.iter(s).collect::<Vec<_>>())?.value())
            })
        });
        def.promise_method("delete", 1, |s, m, a| {
            let name = arg_string(&a, 0, "StorageBucketManager.delete")?;
            let key = m.key()?;
            task(s, &m.page, move |s, page| {
                let catalog = page.local_storage.borrow().buckets();
                catalog.delete(&key, &name).map_err(api_error)?;
                Ok(s.undefined())
            })
        });
    }
}

struct BucketData {
    page: Weak<PageDom>,
    bucket: Arc<Bucket>,
}

struct StorageBucket;
impl JsClass for StorageBucket {
    const NAME: &'static str = "StorageBucket";
    type Data = BucketData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("name", |s, data| Ok(s.string(&data.bucket.name)));
        def.promise_method("persist", 0, persistence);
        def.promise_method("persisted", 0, persistence);
        def.promise_method("estimate", 0, |s, data, _| {
            let bucket = data.bucket.clone();
            task(s, &data.page, move |s, _| {
                let usage = bucket.estimate().map_err(api_error)?;
                let result = s.object();
                result.set_atom("usage", s.number(usage.usage as f64))?;
                result.set_atom("quota", s.number(usage.quota as f64))?;
                Ok(result.value())
            })
        });
        def.promise_method("expires", 0, |s, data, _| {
            let bucket = data.bucket.clone();
            task(s, &data.page, move |s, _| Ok(bucket.expires().map_err(api_error)?.map_or(s.null(), |n| s.number(n))))
        });
        def.promise_method("setExpires", 1, |s, data, a| {
            a.require(1, "StorageBucket.setExpires")?;
            let expiry = finite_number(s, a.get(0))?;
            let bucket = data.bucket.clone();
            task(s, &data.page, move |s, _| {
                bucket.set_expires(expiry).map_err(api_error)?;
                Ok(s.undefined())
            })
        });
        def.getter_this("indexedDB", |s, data, this| {
            endpoint(s, this, "lpBucketIDB", || crate::indexeddb::bucket_factory(s, &owner(&data.page)?, &data.bucket))
        });
        def.getter_this("caches", |s, data, this| {
            endpoint(s, this, "lpBucketCaches", || crate::cache_storage::bucket_caches(s, &owner(&data.page)?, &data.bucket))
        });
        def.promise_method("getDirectory", 0, |s, data, _| {
            let bucket = data.bucket.clone();
            task(s, &data.page, move |s, page| crate::opfs::bucket_directory(s, page, &bucket))
        });
    }
}

fn persistence<'s>(s: &Scope<'s>, data: &BucketData, _: pyro_js::Args<'s>) -> JsResult<Value<'s>> {
    let bucket = data.bucket.clone();
    // As for StorageManager, this browser's ephemeral store cannot grant persistence.
    task(s, &data.page, move |s, _| Ok(s.boolean(bucket.persisted().map_err(api_error)?)))
}

fn endpoint<'s>(
    s: &Scope<'s>, this: Object<'s>, slot: &'static str, make: impl FnOnce() -> JsResult<Value<'s>>,
) -> JsResult<Value<'s>> {
    if let Some(value) = s.get_hidden(this, slot)? { return Ok(value); }
    let value = make()?;
    s.set_hidden(this, slot, value)?;
    Ok(value)
}

pub(crate) fn manager<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if !crate::worker_scope::secure_worker(s).unwrap_or_else(|| crate::secure_context::secure(s, page)) {
        return Ok(s.undefined());
    }
    let holder = crate::worker_scope::module_holder(s, page)?;
    endpoint(s, holder, "lpStorageBuckets", || {
        let data = ManagerData { page: Rc::downgrade(page), key: storage_key(s, page) };
        Ok(s.wrap::<StorageBucketManager>(data, None)?.value())
    })
}

pub(crate) fn register(engine: &Engine) {
    engine.register::<StorageBucketManager>();
    engine.register::<StorageBucket>();
}

pub(crate) fn install_worker(s: &Scope<'_>) -> JsResult<()> {
    // The persist() operation is exposed only on Window.
    if let Some(class) = s.global().get_atom("StorageBucket")?.as_object() {
        if let Some(proto) = class.get_atom("prototype")?.as_object() {
            proto.delete("persist")?;
        }
    }
    Ok(())
}
