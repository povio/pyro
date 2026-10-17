//! In-memory Web Storage. Embedders share a local namespace per browser
//! context and a session namespace per top-level page across navigations.
//! No disk persistence or cross-document `storage` events yet.
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
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

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

struct Namespace {
    indexed_db: crate::indexeddb::Namespace,
    origins: RefCell<FxHashMap<String, Rc<RefCell<Area>>>>,
    quota: usize,
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
        }))
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

pub struct StorageData(Rc<RefCell<Area>>);
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
            data.0.borrow_mut().set(key, value)?;
            Ok(s.undefined())
        });
        def.method("removeItem", 1, |s, data, args| {
            let key = arg_string(&args, 0, "Storage.removeItem")?;
            data.0.borrow_mut().remove(&key);
            Ok(s.undefined())
        });
        def.method("clear", 0, |s, data, _args| {
            let mut area = data.0.borrow_mut();
            area.entries.clear();
            area.bytes = 0;
            Ok(s.undefined())
        });
        def.named(false, |s, data, name| Ok(data.0.borrow().get(name).map(|v| s.string(v))));
        def.named_setter(|_s, data, name, value| {
            let value = dom_string(value)?;
            data.0.borrow_mut().set(name.to_owned(), value)?;
            Ok(true)
        });
        def.named_deleter(|_s, data, name| {
            data.0.borrow_mut().remove(name);
            Ok(true)
        });
        def.property_names(|_s, data| Ok(data.0.borrow().entries.iter().map(|(k, _)| k.clone()).collect()));
    }
}

/// The realm's tuple origin, or `None` when it is opaque. An explicitly
/// inherited realm origin handles about:blank/srcdoc.
pub(crate) fn tuple_origin(s: &Scope<'_>, page: &PageDom) -> Option<String> {
    let url = s.realm().origin().map(str::to_owned).unwrap_or_else(|| {
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

pub(crate) fn window_storage<'s>(s: &Scope<'s>, page: &PageDom, session: bool) -> JsResult<Value<'s>> {
    let area = match tuple_origin(s, page) {
        Some(origin) => {
            let namespace = if session { &page.session_storage } else { &page.local_storage };
            namespace.borrow().area(origin)
        }
        None if page.is_disposed() => return Err(JsError::ContextDestroyed),
        None => page.opaque_storage.area(page, session),
    };
    let key = IdentityKey(page.id() | (1 << 56), area.borrow().id);
    if let Some(object) = s.lookup_identity(key) {
        return Ok(object.value());
    }
    Ok(s.wrap::<Storage>(StorageData(area), Some(key))?.value())
}
