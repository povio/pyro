//! In-memory Web Storage. Embedders share a local namespace per browser
//! context and a session namespace per top-level page across navigations.
//! No disk persistence. A change queues a `storage` event (StorageEvent) on
//! every other page attached to the same namespace whose document has the
//! same origin (`broadcast`); the changing document gets none.
//!
//! Opaque origins (e.g. a CDP-created `about:blank` without an inherited
//! origin, `data:` documents): upstream Lightpanda hands every origin key a
//! bucket, so `localStorage` never throws; Chrome throws SecurityError. For
//! upstream compatibility we give such a document its own ephemeral
//! local/session areas instead (documented Chrome deviation). They are shared
//! only by the worlds of that document — never through a common "null"
//! bucket or with other documents, pages or contexts — and are discarded on
//! document replacement or disposal. A `Storage` object retained across the
//! discard reads as empty (length 0, no keys) and throws InvalidStateError on
//! `setItem`/named assignment; old contents never remain reachable. Other
//! opaque-origin restrictions (e.g. IndexedDB) are unchanged.

use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Scope, Value};
use rustc_hash::FxHashMap;

use crate::page::PageDom;
use crate::util::{arg_string, dom_string, to_u32};

static NEXT_AREA: AtomicU64 = AtomicU64::new(1);
const DEFAULT_QUOTA: usize = 5 * 1024 * 1024;

struct Area {
    id: u64,
    entries: Vec<(String, String)>,
    bytes: usize,
    quota: usize,
    /// Set when an opaque-origin document's area is discarded: contents are
    /// dropped and retained `Storage` wrappers read as empty and reject writes.
    retired: bool,
}

impl Area {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set(&mut self, key: String, value: String) -> JsResult<()> {
        if self.retired {
            return Err(JsError::dom(
                "InvalidStateError",
                "This storage area was discarded with its opaque-origin document.",
            ));
        }
        let index = self.entries.iter().position(|(k, _)| k == &key);
        let old_bytes = index.map_or(0, |i| entry_bytes(&self.entries[i].0, &self.entries[i].1));
        let bytes = self.bytes.saturating_sub(old_bytes).saturating_add(entry_bytes(&key, &value));
        if bytes > self.quota {
            return Err(JsError::dom("QuotaExceededError", "The storage quota has been exceeded."));
        }
        match index {
            Some(i) => self.entries[i].1 = value,
            None => self.entries.push((key, value)),
        }
        self.bytes = bytes;
        Ok(())
    }

    fn remove(&mut self, key: &str) {
        if let Some(i) = self.entries.iter().position(|(k, _)| k == key) {
            let (key, value) = self.entries.remove(i);
            self.bytes -= entry_bytes(&key, &value);
        }
    }
}

fn entry_bytes(key: &str, value: &str) -> usize {
    (key.encode_utf16().count() + value.encode_utf16().count()).saturating_mul(2)
}

/// Entries every new local-storage namespace starts with (the `serve
/// --local-storage` preload): per tuple origin, key/value pairs in order.
/// Built once per process and shared by every browser thread; each context
/// gets its own copy of the entries, so pages never see another context's
/// writes. Seeding bypasses the quota (the operator chose the data); later
/// writes are checked against the quota including the seeded bytes.
#[derive(Clone, Debug, Default)]
pub struct StorageSeed(Arc<SeedEntries>);

/// Per origin (storage key form), the key/value pairs in order.
type SeedEntries = Vec<(String, Vec<(String, String)>)>;

impl StorageSeed {
    /// `origins`: (origin or URL, entries). Each origin is normalized to the
    /// storage key form (`scheme://host[:port]`); an opaque origin such as a
    /// `data:` URL is an error. Repeated origins merge; a repeated key within
    /// one origin keeps the last value.
    pub fn new(origins: SeedEntries) -> Result<Self, String> {
        let mut out: SeedEntries = Vec::new();
        for (origin, entries) in origins {
            let key = seed_origin(&origin)?;
            let slot = match out.iter_mut().find(|(o, _)| *o == key) {
                Some((_, slot)) => slot,
                None => {
                    out.push((key, Vec::new()));
                    &mut out.last_mut().expect("just pushed").1
                }
            };
            for (k, v) in entries {
                match slot.iter_mut().find(|(sk, _)| *sk == k) {
                    Some((_, sv)) => *sv = v,
                    None => slot.push((k, v)),
                }
            }
        }
        Ok(Self(Arc::new(out)))
    }

    pub fn is_empty(&self) -> bool {
        self.0.iter().all(|(_, entries)| entries.is_empty())
    }

    /// (origin, entry count) per seeded origin.
    pub fn summary(&self) -> Vec<(String, usize)> {
        self.0.iter().map(|(o, e)| (o.clone(), e.len())).collect()
    }
}

/// The storage origin key for an origin or URL string.
fn seed_origin(text: &str) -> Result<String, String> {
    let url = url::Url::parse(text.trim()).map_err(|e| format!("'{text}': not an origin or URL ({e})"))?;
    match url.origin() {
        url::Origin::Tuple(..) => Ok(url.origin().ascii_serialization()),
        url::Origin::Opaque(_) => Err(format!("'{text}': opaque origins have no local storage")),
    }
}

struct Namespace {
    indexed_db: crate::indexeddb::Namespace,
    origins: RefCell<FxHashMap<String, Rc<RefCell<Area>>>>,
    quota: usize,
    /// Pages attached to this namespace (`PageDom::set_storage_namespaces`):
    /// the candidates for `storage` events.
    pages: RefCell<Vec<Weak<PageDom>>>,
}

/// Storage partition owned by a browser context (local) or top-level page
/// (session). Clone to share across documents; create a new handle to isolate.
#[derive(Clone)]
pub struct StorageNamespace(Rc<Namespace>);

impl Default for StorageNamespace {
    fn default() -> Self {
        Self::with_quota(DEFAULT_QUOTA)
    }
}

impl StorageNamespace {
    /// Per-origin quota in UTF-16 bytes, including keys and values.
    pub fn with_quota(bytes: usize) -> Self {
        Self(Rc::new(Namespace {
            indexed_db: Default::default(),
            origins: RefCell::new(FxHashMap::default()),
            quota: bytes,
            pages: RefCell::new(Vec::new()),
        }))
    }

    /// A namespace with the default quota holding a copy of `seed`'s entries.
    pub fn seeded(seed: &StorageSeed) -> Self {
        let namespace = Self::default();
        namespace.seed(seed);
        namespace
    }

    /// Writes `seed`'s entries into this namespace (existing keys are
    /// overwritten, other keys kept). No `storage` events: the namespace is
    /// expected to be fresh, and seeding is not a script-visible change.
    pub fn seed(&self, seed: &StorageSeed) {
        for (origin, entries) in seed.0.iter() {
            if entries.is_empty() {
                continue;
            }
            let area = self.area(origin.clone());
            let mut area = area.borrow_mut();
            for (key, value) in entries {
                match area.entries.iter().position(|(k, _)| k == key) {
                    Some(i) => {
                        area.bytes = area.bytes.saturating_sub(entry_bytes(key, &area.entries[i].1));
                        area.entries[i].1 = value.clone();
                    }
                    None => area.entries.push((key.clone(), value.clone())),
                }
                area.bytes = area.bytes.saturating_add(entry_bytes(key, value));
            }
        }
    }

    /// The value stored for `key` under `origin` (storage key form), if any.
    /// Does not create an area for an unknown origin.
    pub fn get(&self, origin: &str, key: &str) -> Option<String> {
        let area = self.0.origins.borrow().get(origin)?.clone();
        let value = area.borrow().get(key).map(str::to_owned);
        value
    }

    /// Registers a page that uses this namespace (for `storage` events).
    pub(crate) fn attach(&self, page: &Rc<PageDom>) {
        let mut pages = self.0.pages.borrow_mut();
        pages.retain(|p| p.strong_count() > 0);
        if !pages.iter().any(|p| p.as_ptr() == Rc::as_ptr(page)) {
            pages.push(Rc::downgrade(page));
        }
    }

    fn same(&self, other: &StorageNamespace) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }

    pub(crate) fn indexed_db(&self) -> crate::indexeddb::Namespace {
        self.0.indexed_db.clone()
    }

    fn area(&self, origin: String) -> Rc<RefCell<Area>> {
        self.0.origins.borrow_mut().entry(origin).or_insert_with(|| self.new_area()).clone()
    }

    /// A fresh area outside the origin map (opaque-origin documents).
    fn new_area(&self) -> Rc<RefCell<Area>> {
        Rc::new(RefCell::new(Area {
            id: NEXT_AREA.fetch_add(1, Ordering::Relaxed),
            entries: Vec::new(),
            bytes: 0,
            quota: self.0.quota,
            retired: false,
        }))
    }
}

/// A `Storage` object: its area, and for a tuple origin what a change must
/// notify (the page holding it, local or session, the origin).
pub struct StorageData(Rc<RefCell<Area>>, Option<Source>);

struct Source {
    page: Weak<PageDom>,
    session: bool,
    origin: String,
}

impl StorageData {
    /// Queues `storage` events for a change: `key` None for clear().
    fn changed(&self, key: Option<&str>, old: Option<String>, new: Option<String>) {
        if let Some(source) = &self.1 {
            broadcast(source, key.map(str::to_owned), old, new);
        }
    }
}

/// HTML "broadcast": every other page sharing the namespace whose document
/// has `origin` gets a trusted `storage` event on its window, in a task.
fn broadcast(source: &Source, key: Option<String>, old: Option<String>, new: Option<String>) {
    let Some(page) = source.page.upgrade() else { return };
    let namespace =
        if source.session { page.session_storage.borrow().clone() } else { page.local_storage.borrow().clone() };
    let url = page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default();
    let targets: Vec<Rc<PageDom>> = namespace.0.pages.borrow().iter().filter_map(Weak::upgrade).collect();
    for target in targets {
        if Rc::ptr_eq(&target, &page) || target.is_disposed() {
            continue;
        }
        let ns = if source.session {
            target.session_storage.borrow().clone()
        } else {
            target.local_storage.borrow().clone()
        };
        if !ns.same(&namespace) || document_origin(&target).as_deref() != Some(source.origin.as_str()) {
            continue;
        }
        let (key, old, new, url, session) = (key.clone(), old.clone(), new.clone(), url.clone(), source.session);
        let weak = Rc::downgrade(&target);
        target.post_task(Box::new(move || {
            let Some(target) = weak.upgrade() else { return };
            if target.is_disposed() {
                return;
            }
            let Some(window) = target.window_target() else { return };
            let event = crate::event::EventCore::trusted(
                &target,
                crate::event::EventInterface::StorageEvent,
                "storage",
                false,
                false,
            );
            {
                let mut f = event.fields.borrow_mut();
                f.storage_key = key;
                f.storage_old = old;
                f.storage_new = new;
                f.storage_url = url;
                f.storage_area = Some(crate::event::StorageAreaRef::Window { session });
            }
            crate::target::fire(&target, window, &event);
        }));
    }
}

/// The tuple origin of a page's current document, if any.
fn document_origin(page: &PageDom) -> Option<String> {
    let url = page.dom.borrow().document_data(page.document()).ok().map(|d| d.url.to_string())?;
    url::Url::parse(&url)
        .ok()
        .map(|url| url.origin())
        .filter(|origin| matches!(origin, url::Origin::Tuple(..)))
        .map(|origin| origin.ascii_serialization())
}

pub struct Storage;

impl JsClass for Storage {
    const NAME: &'static str = "Storage";
    type Data = StorageData;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, data| Ok(s.number(data.0.borrow().entries.len() as f64)));
        def.method("key", 1, |s, data, args| {
            args.require(1, "Storage.key")?;
            let index = to_u32(args.get(0))? as usize;
            Ok(data.0.borrow().entries.get(index).map_or(s.null(), |(key, _)| s.string(key)))
        });
        def.method("getItem", 1, |s, data, args| {
            let key = arg_string(&args, 0, "Storage.getItem")?;
            Ok(data.0.borrow().get(&key).map_or(s.null(), |v| s.string(v)))
        });
        def.method("setItem", 2, |s, data, args| {
            args.require(2, "Storage.setItem")?;
            // Coerce before borrowing; user toString may access the same storage.
            let key = dom_string(args.get(0))?;
            let value = dom_string(args.get(1))?;
            set_item(data, key, value)?;
            Ok(s.undefined())
        });
        def.method("removeItem", 1, |s, data, args| {
            let key = arg_string(&args, 0, "Storage.removeItem")?;
            remove_item(data, &key);
            Ok(s.undefined())
        });
        def.method("clear", 0, |s, data, _args| {
            let had = {
                let mut area = data.0.borrow_mut();
                let had = !area.entries.is_empty();
                area.entries.clear();
                area.bytes = 0;
                had
            };
            if had {
                data.changed(None, None, None);
            }
            Ok(s.undefined())
        });
        def.named(false, |s, data, name| Ok(data.0.borrow().get(name).map(|v| s.string(v))));
        def.named_setter(|_s, data, name, value| {
            let value = dom_string(value)?;
            set_item(data, name.to_owned(), value)?;
            Ok(true)
        });
        def.named_deleter(|_s, data, name| {
            remove_item(data, name);
            Ok(true)
        });
        def.property_names(|_s, data| Ok(data.0.borrow().entries.iter().map(|(k, _)| k.clone()).collect()));
    }
}

/// `setItem`: no event when the value is unchanged.
fn set_item(data: &StorageData, key: String, value: String) -> JsResult<()> {
    let old = data.0.borrow().get(&key).map(str::to_owned);
    if old.as_deref() == Some(value.as_str()) {
        return Ok(());
    }
    data.0.borrow_mut().set(key.clone(), value.clone())?;
    data.changed(Some(&key), old, Some(value));
    Ok(())
}

/// `removeItem`: no event when the key is absent.
fn remove_item(data: &StorageData, key: &str) {
    let old = data.0.borrow().get(key).map(str::to_owned);
    if old.is_some() {
        data.0.borrow_mut().remove(key);
        data.changed(Some(key), old, None);
    }
}

/// The realm's tuple origin, or `None` when it is opaque. An explicitly
/// inherited realm origin handles about:blank/srcdoc.
pub(crate) fn tuple_origin(s: &Scope<'_>, page: &PageDom) -> Option<String> {
    let url = s.realm().origin().unwrap_or_else(|| {
        page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default()
    });
    url::Url::parse(&url)
        .ok()
        .map(|url| url.origin())
        .filter(|origin| matches!(origin, url::Origin::Tuple(..)))
        .map(|origin| origin.ascii_serialization())
}

/// Origin key for origin-partitioned storage; opaque realms must not share a
/// common "null" bucket, so they get SecurityError.
pub(crate) fn storage_origin(s: &Scope<'_>, page: &PageDom) -> JsResult<String> {
    tuple_origin(s, page).ok_or_else(|| JsError::dom("SecurityError", "Storage is unavailable for an opaque origin."))
}

/// Per-document ephemeral areas for an opaque-origin document (see module
/// docs). Keyed to the document so a replacement starts empty.
#[derive(Default)]
pub(crate) struct OpaqueStorage(RefCell<Option<OpaqueAreas>>);

/// (document, local area, session area).
type OpaqueAreas = (lp_dom::NodeId, Rc<RefCell<Area>>, Rc<RefCell<Area>>);

impl OpaqueStorage {
    fn area(&self, page: &PageDom, session: bool) -> Rc<RefCell<Area>> {
        let doc = page.document();
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().is_none_or(|(d, _, _)| *d != doc) {
            let local = page.local_storage.borrow().new_area();
            let session = page.session_storage.borrow().new_area();
            *slot = Some((doc, local, session));
        }
        let (_, local, session_area) = slot.as_ref().expect("set above");
        if session {
            session_area.clone()
        } else {
            local.clone()
        }
    }

    /// Discards the areas (document replacement or page disposal). Wrappers
    /// already handed to script keep their `Rc`, so the areas are emptied and
    /// retired rather than merely detached.
    pub(crate) fn clear(&self) {
        let Some((_, local, session)) = self.0.take() else { return };
        for area in [local, session] {
            let mut area = area.borrow_mut();
            area.entries = Vec::new();
            area.bytes = 0;
            area.retired = true;
        }
    }
}

pub(crate) fn window_storage<'s>(s: &Scope<'s>, page: &Rc<PageDom>, session: bool) -> JsResult<Value<'s>> {
    let (area, source) = match tuple_origin(s, page) {
        Some(origin) => {
            let namespace = if session { &page.session_storage } else { &page.local_storage };
            let area = namespace.borrow().area(origin.clone());
            (area, Some(Source { page: Rc::downgrade(page), session, origin }))
        }
        None if page.is_disposed() => return Err(JsError::ContextDestroyed),
        // Opaque-origin areas belong to one document: no other document sees them.
        None => (page.opaque_storage.area(page, session), None),
    };
    let key = IdentityKey(page.id() | (1 << 56), area.borrow().id);
    if let Some(object) = s.lookup_identity(key) {
        return Ok(object.value());
    }
    Ok(s.wrap::<Storage>(StorageData(area, source), Some(key))?.value())
}
