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
use std::sync::{Arc, Mutex, MutexGuard};

use bytes::Bytes;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;

/// Hidden key of the realm's module on its navigator.
const KEY: &str = "lpCacheStorage";

/// The context partition's caches, by origin, in creation order. Clone to share.
#[derive(Clone, Default)]
pub(crate) struct Namespace(Arc<Mutex<FxHashMap<String, Vec<Named>>>>);

/// A cache under its name.
type Named = (String, CacheRef);

/// One cache. A `Cache` object keeps it after `caches.delete()` dropped its name.
pub type CacheRef = Arc<Mutex<Vec<Entry>>>;

/// A request/response pair, in insertion order.
#[derive(Clone)]
pub struct Entry {
    /// Without its fragment.
    url: String,
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
    body: Option<Bytes>,
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

fn natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let native = s.object();
    // storage(op, name): 'get' (handle or null), 'open', 'has', 'delete', 'keys'.
    let f = s.function("storage", |s, _, a| {
        let page = crate::target::page_of(s)?;
        let (namespace, origin) = origin_caches(s, &page)?;
        let op = a.get(0).to_string()?;
        let name = a.get(1).to_string()?;
        let mut origins = lock(&namespace.0);
        let caches = origins.entry(origin).or_default();
        let found = caches.iter().position(|(n, _)| *n == name);
        match op.as_str() {
            "get" => match found {
                Some(i) => {
                    let cache = caches[i].1.clone();
                    drop(origins);
                    handle(s, cache)
                }
                None => Ok(s.null()),
            },
            "open" => {
                let cache = match found {
                    Some(i) => caches[i].1.clone(),
                    None => {
                        let cache = CacheRef::default();
                        caches.push((name, cache.clone()));
                        cache
                    }
                };
                drop(origins);
                handle(s, cache)
            }
            "has" => Ok(s.boolean(found.is_some())),
            "delete" => Ok(s.boolean(found.map(|i| caches.remove(i)).is_some())),
            _ => {
                let names: Vec<String> = caches.iter().map(|(n, _)| n.clone()).collect();
                drop(origins);
                let items: Vec<Value<'_>> = names.iter().map(|n| s.string(n)).collect();
                Ok(s.array(&items)?.value())
            }
        }
    })?;
    native.set_atom("storage", f.value())?;
    // match(handle | null, url | null, method, headers, ignoreSearch,
    // ignoreMethod, ignoreVary, first): the matching responses, new objects.
    // A null handle searches every cache of the origin, in creation order.
    let f = s.function("match", |s, _, a| {
        let q = query(&a, 1)?;
        let first = a.get(7).to_bool();
        let caches: Vec<CacheRef> = if a.get(0).is_null() {
            let page = crate::target::page_of(s)?;
            let (namespace, origin) = origin_caches(s, &page)?;
            let origins = lock(&namespace.0);
            origins.get(&origin).map(|c| c.iter().map(|(_, c)| c.clone()).collect()).unwrap_or_default()
        } else {
            vec![cache_of(s, a.get(0))?]
        };
        let mut found: Vec<Arc<Stored>> = Vec::new();
        for cache in caches {
            let entries = lock(&cache);
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
    // keys(handle, url | null, method, headers, ignoreSearch, ignoreMethod,
    // ignoreVary): [url, method, [[name, value], ...]] of each matching request.
    let f = s.function("keys", |s, _, a| {
        let cache = cache_of(s, a.get(0))?;
        let q = query(&a, 1)?;
        let found: Vec<Entry> =
            lock(&cache).iter().filter(|e| q.as_ref().is_none_or(|q| q.matches(e))).cloned().collect();
        let mut items = Vec::with_capacity(found.len());
        for e in &found {
            items.push(s.array(&[s.string(&e.url), s.string(&e.method), pairs_value(s, &e.headers)?])?.value());
        }
        Ok(s.array(&items)?.value())
    })?;
    native.set_atom("keys", f.value())?;
    // batch(handle, operations): "Batch Cache Operations", all or nothing.
    // An operation is ['put', url, method, headers, type, responseURL,
    // status, statusText, responseHeaders, redirected, body | null] or
    // ['delete', url, method, headers, ignoreSearch, ignoreMethod,
    // ignoreVary]. Returns whether each delete removed something.
    let f = s.function("batch", |s, _, a| {
        let cache = cache_of(s, a.get(0))?;
        let list = a.get(1).as_object().ok_or_else(|| JsError::Type("operations".into()))?;
        let count = list.get_atom("length")?.to_number()? as u32;
        enum Op {
            Put(Entry),
            Delete(Query),
        }
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
                } else {
                    Some(Bytes::from(crate::encoding::buffer_source_bytes(s, body)?.unwrap_or_default()))
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
                let url = strip_fragment(&url).to_owned();
                ops.push(Op::Put(Entry { url, method, headers, response: Arc::new(response) }));
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
        let mut entries = lock(&cache);
        // Run on a copy: a failing operation leaves the cache as it was.
        let mut next = entries.clone();
        let mut added: Vec<Entry> = Vec::new();
        let mut results = Vec::new();
        for op in ops {
            match op {
                Op::Delete(q) => {
                    let before = next.len();
                    next.retain(|e| !q.matches(e));
                    results.push(next.len() != before);
                }
                Op::Put(entry) => {
                    let q = Query {
                        url: entry.url.clone(),
                        method: entry.method.clone(),
                        headers: entry.headers.clone(),
                        ignore_search: false,
                        ignore_method: false,
                        ignore_vary: false,
                    };
                    // The same request twice in one batch (addAll).
                    if added.iter().any(|e| q.matches(e)) {
                        return Err(JsError::dom(
                            "InvalidStateError",
                            "Failed to execute 'addAll' on 'Cache': duplicate requests",
                        ));
                    }
                    next.retain(|e| !q.matches(e));
                    added.push(entry.clone());
                    next.push(entry);
                }
            }
        }
        *entries = next;
        drop(entries);
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
