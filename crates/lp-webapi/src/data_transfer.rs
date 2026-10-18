//! DataTransfer, DataTransferItemList and DataTransferItem (HTML "drag data
//! store"), carried by `DragEvent.dataTransfer`.
//!
//! There is no native drag-and-drop: every store is script-constructed
//! (`new DataTransfer()`) and so always in read/write mode. Entries are plain
//! native data (strings and file bytes), so a store holds no JS roots; each
//! `files`/`getAsFile()` read wraps a fresh `File`. `items` is the same object
//! for the lifetime of its DataTransfer; item objects are created per access.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bytes::Bytes;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};

use crate::file::{BlobData, File, FileList, FileListData};
use crate::target::page_of;
use crate::util::*;

enum Kind {
    Text { data: String },
    File { bytes: Bytes, name: String, last_modified: f64 },
}

struct Entry {
    id: u64,
    /// The item's type string: the lowercased format for text, the MIME type for files.
    ty: String,
    kind: Kind,
}

#[derive(Default)]
struct Store {
    entries: RefCell<Vec<Entry>>,
    next_id: Cell<u64>,
    drop_effect: RefCell<String>,
    effect_allowed: RefCell<String>,
}

impl Store {
    fn push(&self, ty: String, kind: Kind) -> u64 {
        let id = self.next_id.get() + 1;
        self.next_id.set(id);
        self.entries.borrow_mut().push(Entry { id, ty, kind });
        id
    }

    fn file<'s>(&self, s: &Scope<'s>, e: &Entry) -> JsResult<Option<Object<'s>>> {
        let Kind::File { bytes, name, last_modified } = &e.kind else { return Ok(None) };
        let data = BlobData { bytes: bytes.clone(), mime: e.ty.clone(), file: Some((name.clone(), *last_modified)) };
        Ok(Some(s.wrap::<File>(data, None)?))
    }
}

/// `setData`/`getData`/`clearData` format normalization.
fn normalize_format(format: &str) -> String {
    match format.to_ascii_lowercase().as_str() {
        "text" => "text/plain".into(),
        "url" => "text/uri-list".into(),
        f => f.into(),
    }
}

pub struct DataTransferData {
    store: Rc<Store>,
    items: RefCell<Option<Persistent>>,
}

pub struct DataTransfer;
impl JsClass for DataTransfer {
    const NAME: &'static str = "DataTransfer";
    type Data = DataTransferData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _a| {
            let store = Store::default();
            *store.drop_effect.borrow_mut() = "none".into();
            *store.effect_allowed.borrow_mut() = "none".into();
            s.wrap_new::<DataTransfer>(DataTransferData { store: Rc::new(store), items: RefCell::new(None) }, nt, None)
        });
        def.accessor(
            "dropEffect",
            |s, d| Ok(s.string(&d.store.drop_effect.borrow())),
            |_s, d, v| {
                let v = dom_string(v)?;
                if matches!(v.as_str(), "none" | "copy" | "link" | "move") {
                    *d.store.drop_effect.borrow_mut() = v;
                }
                Ok(())
            },
        );
        def.accessor(
            "effectAllowed",
            |s, d| Ok(s.string(&d.store.effect_allowed.borrow())),
            |_s, d, v| {
                let v = dom_string(v)?;
                if matches!(
                    v.as_str(),
                    "none" | "copy" | "copyLink" | "copyMove" | "link" | "linkMove" | "move" | "all" | "uninitialized"
                ) {
                    *d.store.effect_allowed.borrow_mut() = v;
                }
                Ok(())
            },
        );
        def.getter("items", |s, d| {
            if let Some(p) = d.items.borrow().as_ref() {
                return Ok(p.get(s));
            }
            // The list references only the native store, never this
            // DataTransfer, so caching it here creates no cycle.
            let list = s.wrap::<DataTransferItemList>(ItemListData { store: d.store.clone() }, None)?.value();
            *d.items.borrow_mut() = Some(Persistent::new(s, list));
            Ok(list)
        });
        def.getter("types", |s, d| {
            let entries = d.store.entries.borrow();
            let mut types: Vec<Value<'_>> =
                entries.iter().filter(|e| matches!(e.kind, Kind::Text { .. })).map(|e| s.string(&e.ty)).collect();
            if entries.iter().any(|e| matches!(e.kind, Kind::File { .. })) {
                types.push(s.string("Files"));
            }
            drop(entries);
            let array = s.array(&types)?;
            let freeze = s.global().get("Object")?.as_object().map(|o| o.get("freeze")).transpose()?;
            if let Some(f) = freeze.and_then(|f| f.as_object()) {
                f.call(None, &[array.value()])?;
            }
            Ok(array.value())
        });
        def.getter("files", |s, d| {
            let entries = d.store.entries.borrow();
            let mut files = Vec::new();
            for e in entries.iter() {
                if let Some(f) = d.store.file(s, e)? {
                    files.push(Persistent::new(s, f.value()));
                }
            }
            Ok(s.wrap::<FileList>(FileListData::new(files), None)?.value())
        });
        def.method("getData", 1, |s, d, a| {
            let format = normalize_format(&arg_string(&a, 0, "DataTransfer.getData")?);
            let entries = d.store.entries.borrow();
            let data = entries.iter().find_map(|e| match &e.kind {
                Kind::Text { data } if e.ty == format => Some(data.clone()),
                _ => None,
            });
            let data = data.unwrap_or_default();
            if format == "text/uri-list" && a.get(0).as_string().is_some_and(|f| f.eq_ignore_ascii_case("url")) {
                // "url" returns the first URL in the list.
                let first = data.lines().map(str::trim).find(|l| !l.is_empty() && !l.starts_with('#'));
                return Ok(s.string(first.unwrap_or("")));
            }
            Ok(s.string(&data))
        });
        def.method("setData", 2, |s, d, a| {
            a.require(2, "DataTransfer.setData")?;
            let format = normalize_format(&dom_string(a.get(0))?);
            let data = dom_string(a.get(1))?;
            d.store.entries.borrow_mut().retain(|e| !(matches!(e.kind, Kind::Text { .. }) && e.ty == format));
            d.store.push(format, Kind::Text { data });
            Ok(s.undefined())
        });
        def.method("clearData", 0, |s, d, a| {
            let format = if a.get(0).is_undefined() { None } else { Some(normalize_format(&dom_string(a.get(0))?)) };
            d.store
                .entries
                .borrow_mut()
                .retain(|e| !matches!(e.kind, Kind::Text { .. }) || format.as_ref().is_some_and(|f| *f != e.ty));
            Ok(s.undefined())
        });
        def.method("setDragImage", 3, |s, _d, a| {
            a.require(3, "DataTransfer.setDragImage")?;
            Ok(s.undefined())
        });
    }
}

pub struct ItemListData {
    store: Rc<Store>,
}

fn item_value<'s>(s: &Scope<'s>, store: &Rc<Store>, id: u64) -> JsResult<Value<'s>> {
    Ok(s.wrap::<DataTransferItem>(ItemData { store: store.clone(), id }, None)?.value())
}

pub struct DataTransferItemList;
impl JsClass for DataTransferItemList {
    const NAME: &'static str = "DataTransferItemList";
    type Data = ItemListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, l| Ok(s.number(l.store.entries.borrow().len() as f64)));
        def.indexed(|s, l, i| {
            let id = l.store.entries.borrow().get(i as usize).map(|e| e.id);
            id.map(|id| item_value(s, &l.store, id)).transpose()
        });
        def.method("add", 1, |s, l, a| {
            a.require(1, "DataTransferItemList.add")?;
            if let Some(f) = s.unwrap::<File>(a.get(0)) {
                let (name, last_modified) = f.file.clone().unwrap_or_default();
                let id = l.store.push(f.mime.clone(), Kind::File { bytes: f.bytes.clone(), name, last_modified });
                return item_value(s, &l.store, id);
            }
            if a.get(1).is_undefined() {
                return Err(JsError::Type(
                    "Failed to execute 'add' on 'DataTransferItemList': parameter 1 is not of type 'File'.".into(),
                ));
            }
            let data = dom_string(a.get(0))?;
            let ty = dom_string(a.get(1))?.to_ascii_lowercase();
            let exists = l.store.entries.borrow().iter().any(|e| matches!(e.kind, Kind::Text { .. }) && e.ty == ty);
            if exists {
                return Err(JsError::dom("NotSupportedError", format!("An item already exists for type '{ty}'.")));
            }
            let id = l.store.push(ty, Kind::Text { data });
            item_value(s, &l.store, id)
        });
        def.method("remove", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            let mut entries = l.store.entries.borrow_mut();
            if i < entries.len() {
                entries.remove(i);
            }
            Ok(s.undefined())
        });
        def.method("clear", 0, |s, l, _a| {
            l.store.entries.borrow_mut().clear();
            Ok(s.undefined())
        });
        def.indexed_iterable();
    }
}

/// An item stays bound to its entry; once the entry is removed, `kind` and
/// `type` read as empty and the accessors return nothing.
pub struct ItemData {
    store: Rc<Store>,
    id: u64,
}

impl ItemData {
    fn with<R>(&self, f: impl FnOnce(&Entry) -> R) -> Option<R> {
        self.store.entries.borrow().iter().find(|e| e.id == self.id).map(f)
    }
}

pub struct DataTransferItem;
impl JsClass for DataTransferItem {
    const NAME: &'static str = "DataTransferItem";
    type Data = ItemData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("kind", |s, it| {
            let kind = it.with(|e| if matches!(e.kind, Kind::File { .. }) { "file" } else { "string" });
            Ok(s.string(kind.unwrap_or("")))
        });
        def.getter("type", |s, it| Ok(s.string(&it.with(|e| e.ty.clone()).unwrap_or_default())));
        def.method("getAsFile", 0, |s, it, _a| {
            let entries = it.store.entries.borrow();
            let file = match entries.iter().find(|e| e.id == it.id) {
                Some(e) => it.store.file(s, e)?,
                None => None,
            };
            Ok(file.map_or(s.null(), |f| f.value()))
        });
        def.method("getAsString", 1, |s, it, a| {
            let callback = a.get(0);
            let data = it.with(|e| match &e.kind {
                Kind::Text { data } => Some(data.clone()),
                Kind::File { .. } => None,
            });
            let (Some(Some(data)), true) = (data, callback.is_function()) else { return Ok(s.undefined()) };
            // "Queue a task to invoke callback with the data"; the callback is
            // rooted only until that task runs or the page goes away.
            let page = page_of(s)?;
            let realm = s.realm().downgrade();
            let callback = Persistent::new(s, callback);
            page.post_task(Box::new(move || {
                let Some(realm) = realm.upgrade() else { return };
                let Ok(s) = realm.scope() else { return };
                let Some(f) = callback.get(&s).as_object() else { return };
                if let Err(err) = f.call(None, &[s.string(&data)]) {
                    s.report_exception(&err);
                }
            }));
            Ok(s.undefined())
        });
    }
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<DataTransfer>();
    engine.register::<DataTransferItemList>();
    engine.register::<DataTransferItem>();
}
