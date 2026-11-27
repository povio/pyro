//! Cache Storage (Service Workers spec, "Cache" and "CacheStorage"), in
//! memory.
//!
//! Caches live natively in the browser context's local storage partition,
//! per origin ([`Namespace`], shared like IndexedDB's with the context's
//! documents and the dedicated and service workers they start), so a page and
//! its service worker see the same caches. Nothing is written to disk; a new
//! context starts empty.
//!
//! The API objects are cache_storage.js, compiled on a realm's first use of
//! `caches`, `Cache` or `CacheStorage`: a realm that never touches them pays
//! only the accessors. The natives here hold the entries and run the "Query
//! Cache" and "Batch Cache Operations" algorithms; a Response read back from a
//! cache is rebuilt with its stored type, URL, status and headers.
//!
//! An opaque origin (a sandboxed frame without `allow-same-origin`) has no
//! cache storage: `caches` throws a SecurityError there, as in Chromium.
use std::rc::Rc;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use bytes::Bytes;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::storage_buckets::catalog::{Bucket, Endpoint, Error as BucketError, Lease};

/// Hidden key of the realm's module on its navigator.
const KEY: &str = "lpCacheStorage";

/// The context partition's caches, by origin, in creation order. Clone to share.
#[derive(Clone, Default)]
pub(crate) struct Namespace {
    directory: Arc<Mutex<CacheDirectory>>,
    lease: Option<Lease>,
}

impl Namespace {
    pub(crate) fn for_bucket(lease: Lease) -> Self {
        Self { lease: Some(lease), ..Self::default() }
    }

    /// Revoke every cache of this generation, including caches that were
    /// removed by name but still have a live Cache object.
    pub(crate) fn retire(&self) {
        let directory = std::mem::take(&mut *lock(&self.directory));
        for cache in directory.live.into_iter().filter_map(|weak| weak.upgrade()) {
            *lock(&cache.entries) = Vec::new();
        }
    }
}

#[derive(Default)]
struct CacheDirectory {
    origins: FxHashMap<String, Vec<Named>>,
    /// Only named-bucket caches need eager clearing when a generation ends.
    live: Vec<Weak<CacheData>>,
}

impl CacheDirectory {
    fn open(&mut self, origin: String, name: String, lease: Option<&Lease>) -> CacheRef {
        let caches = self.origins.entry(origin).or_default();
        if let Some((_, cache)) = caches.iter().find(|(existing, _)| existing == &name) {
            return cache.clone();
        }
        let cache = CacheData::new(lease.cloned());
        if lease.is_some() {
            // Compact at growth boundaries so churn through unlinked caches
            // doesn't keep growing the weak registry or scan it on every open.
            if self.live.len() == self.live.capacity() {
                self.live.retain(|weak| weak.strong_count() != 0);
            }
            self.live.push(Arc::downgrade(&cache));
        }
        caches.push((name, cache.clone()));
        cache
    }
}

/// A cache under its name.
type Named = (String, CacheRef);

/// One cache. A `Cache` object keeps it after `caches.delete()` dropped its name.
pub type CacheRef = Arc<CacheData>;

pub struct CacheData {
    entries: Mutex<Vec<Entry>>,
    lease: Option<Lease>,
}

impl CacheData {
    fn new(lease: Option<Lease>) -> CacheRef {
        Arc::new(Self { entries: Mutex::new(Vec::new()), lease })
    }

    fn check(&self) -> JsResult<()> {
        if let Some(lease) = &self.lease {
            lease.check().map_err(cache_error)?;
        }
        Ok(())
    }
}

impl Drop for CacheData {
    fn drop(&mut self) {
        // Removing a name does not empty an already-open Cache. Its bytes
        // remain charged until the last named or script reference goes away.
        if let Some(lease) = &self.lease {
            let entries = self.entries.get_mut().unwrap_or_else(|e| e.into_inner());
            let _ = lease.charge(Endpoint::Caches, entries.iter().map(entry_size).sum(), 0);
        }
    }
}

enum Op {
    Put(Entry),
    Delete(Query),
}

impl CacheData {
    fn batch(&self, ops: Vec<Op>) -> JsResult<Vec<bool>> {
        let same_request = |entry: &Entry| Query {
            url: entry.url.clone(),
            method: entry.method.clone(),
            headers: entry.headers.clone(),
            ignore_search: false,
            ignore_method: false,
            ignore_vary: false,
        };
        // Reject duplicate addAll requests before changing any stored entry.
        let mut puts: Vec<&Entry> = Vec::new();
        for op in &ops {
            if let Op::Put(entry) = op {
                let q = same_request(entry);
                if puts.iter().any(|e| q.matches(e)) {
                    return Err(JsError::dom(
                        "InvalidStateError",
                        "Failed to execute 'addAll' on 'Cache': duplicate requests",
                    ));
                }
                puts.push(entry);
            }
        }
        let mut live = lock(&self.entries);
        self.check()?;
        // Quota is shared with the bucket's other endpoints. Stage only
        // named-bucket batches; the default path continues to mutate in place.
        let mut staged;
        let entries = if self.lease.is_some() {
            staged = live.clone();
            &mut staged
        } else {
            &mut *live
        };
        let mut results = Vec::new();
        for op in ops {
            match op {
                Op::Delete(q) => {
                    let before = entries.len();
                    entries.retain(|e| !q.matches(e));
                    results.push(entries.len() != before);
                }
                Op::Put(entry) => {
                    let q = same_request(&entry);
                    entries.retain(|e| !q.matches(e));
                    entries.push(entry);
                }
            }
        }
        if let Some(lease) = &self.lease {
            let total = entries.iter().map(entry_size).sum();
            let next = std::mem::take(entries);
            lease.charge(Endpoint::Caches, live.iter().map(entry_size).sum(), total).map_err(cache_error)?;
            *live = next;
        }
        drop(live);
        Ok(results)
    }
}

fn cache_error(e: BucketError) -> JsError {
    if matches!(e, BucketError::QuotaExceeded) {
        JsError::dom("QuotaExceededError", "The storage bucket quota has been exceeded.")
    } else {
        JsError::dom("NotFoundError", "The cache's storage bucket no longer exists.")
    }
}

/// A request/response pair, in insertion order.
#[derive(Clone)]
pub struct Entry {
    /// Without its fragment: what matching and replacing compare.
    url: String,
    /// The request's URL as `put` got it, fragment included (what `keys()` returns).
    request_url: String,
    method: String,
    headers: Vec<(String, String)>,
    response: Arc<Stored>,
}

/// A response as `cache.put` read it.
struct Stored {
    /// Its `type`: "default", "basic", "cors", "opaque", "opaqueredirect" or "error".
    kind: String,
    url: String,
    status: u16,
    status_text: String,
    headers: Vec<(String, String)>,
    redirected: bool,
    /// In the pieces the network delivered it in, which whatever else keeps
    /// the response's bytes (the protocol's body store) shares.
    body: Option<Vec<Bytes>>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The not-exposed handle a `Cache` object holds.
pub struct CacheHandle;
impl JsClass for CacheHandle {
    const NAME: &'static str = "CacheHandle";
    type Data = CacheRef;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
    }
}

/// A response body as `cache.put` reads it (`ReadKind::Chunks`): the pieces
/// themselves, not a copy in a script buffer.
pub struct StoredBody;
impl JsClass for StoredBody {
    const NAME: &'static str = "StoredBody";
    type Data = Vec<Bytes>;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
    }
}

/// The parts of a request "Query Cache" compares.
struct Query {
    url: String,
    method: String,
    headers: Vec<(String, String)>,
    ignore_search: bool,
    ignore_method: bool,
    ignore_vary: bool,
}

fn strip_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

fn strip_search(url: &str) -> &str {
    strip_fragment(url).split('?').next().unwrap_or(url)
}

/// The combined value of header `name` (lower case) in `headers`.
fn header(headers: &[(String, String)], name: &str) -> Option<String> {
    let mut out: Option<String> = None;
    for (_, v) in headers.iter().filter(|(n, _)| n.eq_ignore_ascii_case(name)) {
        match &mut out {
            Some(o) => {
                o.push_str(", ");
                o.push_str(v);
            }
            None => out = Some(v.clone()),
        }
    }
    out
}

fn vary_names(headers: &[(String, String)]) -> impl Iterator<Item = String> + '_ {
    headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("vary"))
        .flat_map(|(_, v)| v.split(','))
        .map(|n| n.trim().to_ascii_lowercase())
        .filter(|n| !n.is_empty())
}

impl Query {
    /// "Request matches cached item".
    fn matches(&self, entry: &Entry) -> bool {
        if !self.ignore_method && self.method != "GET" {
            return false;
        }
        let same = if self.ignore_search {
            strip_search(&self.url) == strip_search(&entry.url)
        } else {
            strip_fragment(&self.url) == entry.url
        };
        if !same {
            return false;
        }
        if self.ignore_vary {
            return true;
        }
        for name in vary_names(&entry.response.headers) {
            if name == "*" || header(&entry.headers, &name) != header(&self.headers, &name) {
                return false;
            }
        }
        true
    }
}

/// Logical stored bytes, independent of allocator and response sharing.
fn entry_size(entry: &Entry) -> u64 {
    fn header_size(headers: &[(String, String)]) -> usize {
        headers.iter().map(|(name, value)| name.len() + value.len()).sum()
    }
    let response = &entry.response;
    let metadata = entry.url.len() + entry.method.len() + header_size(&entry.headers)
        + response.kind.len() + response.url.len() + response.status_text.len() + header_size(&response.headers);
    metadata as u64 + response.body.iter().flatten().map(|chunk| chunk.len() as u64).sum::<u64>()
}

fn pairs(value: Value<'_>) -> JsResult<Vec<(String, String)>> {
    let Some(list) = value.as_object() else { return Ok(Vec::new()) };
    let count = list.get_atom("length")?.to_number()? as u32;
    let mut out = Vec::with_capacity(count as usize / 2);
    let mut i = 0;
    while i + 1 < count {
        out.push((list.get_index(i)?.to_string()?, list.get_index(i + 1)?.to_string()?));
        i += 2;
    }
    Ok(out)
}

fn pairs_value<'s>(s: &Scope<'s>, headers: &[(String, String)]) -> JsResult<Value<'s>> {
    let mut items = Vec::with_capacity(headers.len());
    for (n, v) in headers {
        items.push(s.array(&[s.string(n), s.string(v)])?.value());
    }
    Ok(s.array(&items)?.value())
}

/// A query from natives' arguments `(url, method, headers, ignoreSearch,
/// ignoreMethod, ignoreVary)` at `at`; `None` for an undefined/null URL (every entry).
fn query(a: &pyro_js::Args<'_>, at: usize) -> JsResult<Option<Query>> {
    let url = a.get(at);
    if url.is_nullish() {
        return Ok(None);
    }
    Ok(Some(Query {
        url: url.to_string()?,
        method: a.get(at + 1).to_string()?,
        headers: pairs(a.get(at + 2))?,
        ignore_search: a.get(at + 3).to_bool(),
        ignore_method: a.get(at + 4).to_bool(),
        ignore_vary: a.get(at + 5).to_bool(),
    }))
}

fn response<'s>(s: &Scope<'s>, stored: &Stored) -> JsResult<Value<'s>> {
    crate::response::stored(
        s,
        &stored.kind,
        &stored.url,
        stored.status,
        stored.status_text.clone(),
        stored.headers.clone(),
        stored.redirected,
        stored.body.clone(),
    )
}

fn origin_caches(s: &Scope<'_>, page: &PageDom) -> JsResult<(Namespace, String)> {
    let origin = crate::storage::storage_origin(s, page)?;
    Ok((page.local_storage.borrow().caches(), origin))
}

fn handle<'s>(s: &Scope<'s>, cache: CacheRef) -> JsResult<Value<'s>> {
    Ok(s.wrap::<CacheHandle>(cache, None)?.value())
}

fn cache_of(s: &Scope<'_>, value: Value<'_>) -> JsResult<CacheRef> {
    s.unwrap::<CacheHandle>(value).map(|c| (*c).clone()).ok_or_else(|| JsError::Type("Illegal invocation".into()))
}

/// A named bucket supplies its private namespace and origin. The default
/// caches object looks up the current page's default storage as before.
fn storage_natives<'s>(s: &Scope<'s>, bucket: Option<(Namespace, String)>) -> JsResult<Object<'s>> {
    let native = s.object();
    // storage(op, name): 'get' (handle or null), 'open', 'has', 'delete', 'keys'.
    let storage_bucket = bucket.clone();
    let f = s.function("storage", move |s, _, a| {
        let page = crate::target::page_of(s)?;
        let (namespace, origin) = match &storage_bucket { Some(pair) => pair.clone(), None => origin_caches(s, &page)? };
        let op = a.get(0).to_string()?;
        let name = a.get(1).to_string()?;
        let mut directory = lock(&namespace.directory);
        // An already obtained CacheStorage remains a view of the retired
        // generation. Queries see no names and it cannot create new ones.
        if namespace.lease.as_ref().is_some_and(|lease| lease.check().is_err()) {
            return match op.as_str() {
                "open" => Err(JsError::dom("UnknownError", "The storage bucket no longer exists.")),
                "get" => Ok(s.null()),
                "has" | "delete" => Ok(s.boolean(false)),
                _ => Ok(s.array(&[])?.value()),
            };
        }
        if op == "open" {
            let cache = directory.open(origin, name, namespace.lease.as_ref());
            drop(directory);
            return handle(s, cache);
        }
        let caches = directory.origins.entry(origin).or_default();
        let found = caches.iter().position(|(n, _)| *n == name);
        match op.as_str() {
            "get" => match found {
                Some(i) => {
                    let cache = caches[i].1.clone();
                    drop(directory);
                    handle(s, cache)
                }
                None => Ok(s.null()),
            },
            "has" => Ok(s.boolean(found.is_some())),
            "delete" => Ok(s.boolean(found.map(|i| caches.remove(i)).is_some())),
            _ => {
                let names: Vec<String> = caches.iter().map(|(n, _)| n.clone()).collect();
                drop(directory);
                let items: Vec<Value<'_>> = names.iter().map(|n| s.string(n)).collect();
                Ok(s.array(&items)?.value())
            }
        }
    })?;
    native.set_atom("storage", f.value())?;
    // match(handle | null, url | null, method, headers, ignoreSearch,
    // ignoreMethod, ignoreVary, first): the matching responses, new objects.
    // A null handle searches every cache of the origin, in creation order.
    let f = s.function("match", move |s, _, a| {
        let q = query(&a, 1)?;
        let first = a.get(7).to_bool();
        let caches: Vec<CacheRef> = if a.get(0).is_null() {
            let page = crate::target::page_of(s)?;
            let (namespace, origin) = match &bucket { Some(pair) => pair.clone(), None => origin_caches(s, &page)? };
            let directory = lock(&namespace.directory);
            if namespace.lease.as_ref().is_some_and(|lease| lease.check().is_err()) {
                return Ok(s.array(&[])?.value());
            }
            directory.origins.get(&origin).map(|c| c.iter().map(|(_, c)| c.clone()).collect()).unwrap_or_default()
        } else {
            vec![cache_of(s, a.get(0))?]
        };
        let mut found: Vec<Arc<Stored>> = Vec::new();
        for cache in caches {
            let entries = lock(&cache.entries);
            cache.check()?;
            for entry in entries.iter().filter(|e| q.as_ref().is_none_or(|q| q.matches(e))) {
                found.push(entry.response.clone());
                if first {
                    break;
                }
            }
            if first && !found.is_empty() {
                break;
            }
        }
        let mut items = Vec::with_capacity(found.len());
        for stored in &found {
            items.push(response(s, stored)?);
        }
        Ok(s.array(&items)?.value())
    })?;
    native.set_atom("match", f.value())?;
    Ok(native)
}

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = storage_natives(s, None)?;
    // keys(handle, url | null, method, headers, ignoreSearch, ignoreMethod,
    // ignoreVary): [url, method, [[name, value], ...]] of each matching request.
    let f = s.function("keys", |s, _, a| {
        let cache = cache_of(s, a.get(0))?;
        let q = query(&a, 1)?;
        let found: Vec<Entry> = {
            let entries = lock(&cache.entries);
            cache.check()?;
            entries.iter().filter(|e| q.as_ref().is_none_or(|q| q.matches(e))).cloned().collect()
        };
        let mut items = Vec::with_capacity(found.len());
        for e in &found {
            items.push(s.array(&[s.string(&e.request_url), s.string(&e.method), pairs_value(s, &e.headers)?])?.value());
        }
        Ok(s.array(&items)?.value())
    })?;
    native.set_atom("keys", f.value())?;
    // read(response): a promise of the response's body for a 'put' (a
    // StoredBody, or an ArrayBuffer for a script's stream), or null for a
    // null body; the body is used afterwards.
    let f = s.function("read", |s, _, a| crate::response::read_for_cache(s, a.get(0)))?;
    native.set_atom("read", f.value())?;
    // batch(handle, operations): "Batch Cache Operations", all or nothing.
    // An operation is ['put', url, method, headers, type, responseURL,
    // status, statusText, responseHeaders, redirected, body | null] (body:
    // what `read` resolved with) or
    // ['delete', url, method, headers, ignoreSearch, ignoreMethod,
    // ignoreVary]. Returns whether each delete removed something.
    let f = s.function("batch", |s, _, a| {
        let cache = cache_of(s, a.get(0))?;
        let list = a.get(1).as_object().ok_or_else(|| JsError::Type("operations".into()))?;
        let count = list.get_atom("length")?.to_number()? as u32;
        let mut ops = Vec::with_capacity(count as usize);
        for i in 0..count {
            let op = list.get_index(i)?.as_object().ok_or_else(|| JsError::Type("operation".into()))?;
            let at = |n: u32| op.get_index(n);
            let url = at(1)?.to_string()?;
            let method = at(2)?.to_string()?;
            let headers = pairs(at(3)?)?;
            if at(0)?.to_string()? == "put" {
                let body = at(10)?;
                let body = if body.is_nullish() {
                    None
                } else if let Some(chunks) = s.unwrap::<StoredBody>(body) {
                    Some(chunks.clone())
                } else {
                    Some(vec![Bytes::from(crate::encoding::buffer_source_bytes(s, body)?.unwrap_or_default())])
                };
                let response = Stored {
                    kind: at(4)?.to_string()?,
                    url: at(5)?.to_string()?,
                    status: at(6)?.to_number()? as u16,
                    status_text: at(7)?.to_string()?,
                    headers: pairs(at(8)?)?,
                    redirected: at(9)?.to_bool(),
                    body,
                };
                let request_url = url.clone();
                let url = strip_fragment(&url).to_owned();
                ops.push(Op::Put(Entry { url, request_url, method, headers, response: Arc::new(response) }));
            } else {
                ops.push(Op::Delete(Query {
                    url,
                    method,
                    headers,
                    ignore_search: at(4)?.to_bool(),
                    ignore_method: at(5)?.to_bool(),
                    ignore_vary: at(6)?.to_bool(),
                }));
            }
        }
        let results = cache.batch(ops)?;
        let items: Vec<Value<'_>> = results.iter().map(|r| s.boolean(*r)).collect();
        Ok(s.array(&items)?.value())
    })?;
    native.set_atom("batch", f.value())?;
    Ok(native)
}

fn unavailable() -> JsError {
    JsError::Type("cache storage is unavailable in this realm".into())
}

/// This realm's module, made on first use.
fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let native = natives(s)?;
    let factory = s.eval(include_str!("cache_storage.js"), Some("pyro://internal/cache-storage.js"), 1)?;
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[native.value()])?;
    s.set_hidden(holder, KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `caches` of a window or worker: the realm's `CacheStorage` in a secure
/// context, undefined elsewhere (`[SecureContext]`); a SecurityError for an
/// opaque origin.
pub(crate) fn caches<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    let Some(origin) = crate::storage::tuple_origin(s, page) else {
        return Err(JsError::dom(
            "SecurityError",
            "Failed to read the 'caches' property: Cache storage is disabled because the context is sandboxed and lacks the 'allow-same-origin' flag.",
        ));
    };
    if !pyro_net::Url::parse(&origin).is_ok_and(|u| pyro_net::is_potentially_trustworthy_url(&u)) {
        return Ok(s.undefined());
    }
    module(s, page)?.get_atom("caches")
}

/// A new CacheStorage wrapper, using this realm's normal CacheStorage and
/// Cache prototypes. Its caller keeps it on its owning StorageBucket.
pub(crate) fn bucket_caches<'s>(s: &Scope<'s>, page: &Rc<PageDom>, bucket: &Bucket) -> JsResult<Value<'s>> {
    let native = storage_natives(s, Some((bucket.caches.clone(), bucket.origin.clone())))?;
    let make = module(s, page)?.get_atom("bucketCaches")?.as_object().ok_or_else(unavailable)?;
    make.call(None, &[native.value()])
}

/// Adds the lazy `Cache` and `CacheStorage` interface names to a global.
pub(crate) fn define_globals<C: JsClass>(def: &mut ClassDef<C>)
where
    C::Data: crate::indexeddb::PageOf,
{
    use crate::indexeddb::PageOf;
    macro_rules! lazy {
        ($($name:literal),+) => {$(
            def.global_value_accessor(
                $name,
                |s, g| {
                    let page = PageOf::page_of(g);
                    let value = module(s, page)?.get($name)?;
                    crate::lazy_module::replace(s, page, $name, value)?;
                    Ok(value)
                },
                |s, g, value| crate::lazy_module::replace(s, PageOf::page_of(g), $name, value),
            );
        )+};
    }
    lazy!("Cache", "CacheStorage");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage_buckets::catalog::{Catalog, Options, StorageKey};

    fn key() -> StorageKey {
        StorageKey {
            origin: "https://example.test".into(),
            top_level_site: Some("https://example.test".into()),
            cross_site_ancestor: false,
        }
    }

    fn put(url: &str, body: &'static [u8]) -> Op {
        Op::Put(Entry {
            url: url.into(),
            request_url: url.into(),
            method: "GET".into(),
            headers: Vec::new(),
            response: Arc::new(Stored {
                kind: "basic".into(),
                url: url.into(),
                status: 200,
                status_text: "OK".into(),
                headers: Vec::new(),
                redirected: false,
                body: Some(vec![Bytes::from_static(body)]),
            }),
        })
    }

    #[test]
    fn named_cache_quota_commits_whole_batches_and_refunds_replacements() {
        let catalog = Catalog::default();
        let bucket = catalog.open(&key(), "inbox", Options { quota: Some(40), ..Default::default() }).unwrap();
        let cache = CacheData::new(Some(bucket.lease.clone()));
        cache.batch(vec![put("/a", b"123")]).unwrap();
        let first = bucket.estimate().unwrap().usage;
        cache.batch(vec![put("/a", b"1")]).unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, first - 2);

        assert!(cache.batch(vec![put("/new", b"x"), put("/too-big", b"abcdefghijklmnopqrstuvwxyz")]).is_err());
        assert_eq!(bucket.estimate().unwrap().usage, first - 2);
        let entries = lock(&cache.entries);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "/a");
        drop(entries);
        cache.batch(vec![Op::Delete(Query {
            url: "/a".into(), method: "GET".into(), headers: Vec::new(),
            ignore_method: false, ignore_search: false, ignore_vary: false,
        })]).unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, 0);
    }

    #[test]
    fn unlinked_cache_stays_charged_and_old_handles_cannot_write_to_recreated_bucket() {
        let catalog = Catalog::default();
        let key = key();
        let bucket = catalog.open(&key, "inbox", Default::default()).unwrap();
        let open = |name: &str| lock(&bucket.caches.directory).open(
            key.origin.clone(), name.into(), Some(&bucket.lease),
        );
        let unlink = || lock(&bucket.caches.directory).origins.get_mut(&key.origin).unwrap().clear();
        let cache = open("old");
        cache.batch(vec![put("/a", b"1")]).unwrap();
        unlink();
        cache.batch(vec![put("/b", b"2")]).unwrap();
        assert!(bucket.estimate().unwrap().usage > 0);
        drop(cache);
        assert_eq!(bucket.estimate().unwrap().usage, 0);

        let orphan = open("keep");
        orphan.batch(vec![put("/orphan", b"old bytes")]).unwrap();
        unlink();
        let current = open("keep");
        assert!(lock(&current.entries).is_empty());
        current.batch(vec![put("/current", b"new bytes")]).unwrap();
        assert_eq!(lock(&orphan.entries).len(), 1);
        catalog.delete(&key, "inbox").unwrap();
        let reopened = catalog.open(&key, "inbox", Default::default()).unwrap();
        for cache in [orphan, current] {
            assert!(lock(&cache.entries).is_empty());
            assert!(cache.check().is_err());
            assert!(cache.batch(vec![put("/stale", b"2")]).is_err());
        }
        assert_eq!(reopened.estimate().unwrap().usage, 0);
    }
}
