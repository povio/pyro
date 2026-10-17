//! In-memory Web Storage. Embedders share a local namespace per browser
//! context and a session namespace per top-level page across navigations.
//! No disk persistence or cross-document `storage` events yet.

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
}

impl Area {
    fn get(&self, key: &str) -> Option<&str> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    fn set(&mut self, key: String, value: String) -> JsResult<()> {
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
        Self(Rc::new(Namespace { origins: RefCell::new(FxHashMap::default()), quota: bytes }))
    }

    fn area(&self, origin: String) -> Rc<RefCell<Area>> {
        self.0
            .origins
            .borrow_mut()
            .entry(origin)
            .or_insert_with(|| {
                Rc::new(RefCell::new(Area {
                    id: NEXT_AREA.fetch_add(1, Ordering::Relaxed),
                    entries: Vec::new(),
                    bytes: 0,
                    quota: self.0.quota,
                }))
            })
            .clone()
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

pub(crate) fn window_storage<'s>(s: &Scope<'s>, page: &PageDom, session: bool) -> JsResult<Value<'s>> {
    // An explicitly inherited realm origin handles about:blank/srcdoc. Opaque
    // realms cannot access storage; they must not share a common "null" bucket.
    let url = s.realm().origin().map(str::to_owned).unwrap_or_else(|| {
        page.dom.borrow().document_data(page.document()).map(|d| d.url.to_string()).unwrap_or_default()
    });
    let origin = url::Url::parse(&url)
        .ok()
        .map(|url| url.origin())
        .filter(|origin| matches!(origin, url::Origin::Tuple(..)))
        .ok_or_else(|| JsError::dom("SecurityError", "Storage is unavailable for an opaque origin."))?
        .ascii_serialization();
    let namespace = if session { &page.session_storage } else { &page.local_storage };
    let area = namespace.borrow().area(origin);
    let key = IdentityKey(page.id() | (1 << 56), area.borrow().id);
    if let Some(object) = s.lookup_identity(key) {
        return Ok(object.value());
    }
    Ok(s.wrap::<Storage>(StorageData(area), Some(key))?.value())
}
