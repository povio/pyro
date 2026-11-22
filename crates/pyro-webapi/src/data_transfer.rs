//! DataTransfer, DataTransferItemList and DataTransferItem (HTML "drag data
//! store"), carried by `DragEvent.dataTransfer`.
//!
//! Script-constructed stores (`new DataTransfer()`) are always in read/write
//! mode. Native drag-and-drop (`crate::input` drag sessions) creates stores
//! through `NativeTransfer` and moves them between the spec's read/write,
//! read-only, protected and disabled modes around each event. Entries are plain
//! native data (strings and file bytes), so a store holds no JS roots; each
//! `files`/`getAsFile()` read wraps a fresh `File`. `items` is the same object
//! for the lifetime of its DataTransfer; item objects are created per access.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use bytes::Bytes;
use pyro_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value, WeakRealm};

use crate::file::{BlobData, File, FileListBuilder};
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

/// HTML "drag data store mode". `Disabled` is a store no longer associated
/// with the event that exposed it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    #[default]
    ReadWrite,
    ReadOnly,
    Protected,
    Disabled,
}

#[derive(Default)]
struct Store {
    entries: RefCell<Vec<Entry>>,
    next_id: Cell<u64>,
    drop_effect: RefCell<String>,
    /// Script assigned a valid dropEffect (Blink tracks an "uninitialized"
    /// drop effect that still reads as "none").
    drop_effect_assigned: Cell<bool>,
    effect_allowed: RefCell<String>,
    mode: Cell<Mode>,
}

impl Store {
    fn writable(&self) -> bool {
        self.mode.get() == Mode::ReadWrite
    }

    /// getData and string/file contents are readable in read/write and read-only mode.
    fn readable(&self) -> bool {
        matches!(self.mode.get(), Mode::ReadWrite | Mode::ReadOnly)
    }

    /// Types and the item list are visible unless the store is disabled.
    fn listed(&self) -> bool {
        self.mode.get() != Mode::Disabled
    }

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

/// The FileList `files` returned last and the file entry ids it was made from.
const FILES_LIST: &str = "__lpDataTransferFiles";
const FILES_KEY: &str = "__lpDataTransferFilesKey";

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
    pub(crate) reference: DataTransferRef,
}

/// Non-rooting identity. Every JS owner must also hold a traced wrapper edge.
#[derive(Clone)]
pub(crate) struct DataTransferRef {
    realm: WeakRealm,
    identity: IdentityKey,
}

impl DataTransferRef {
    pub(crate) fn get<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let owner = realm.scope()?;
        let wrapper = owner.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
        // SAFETY: the caller's live receiver traces this same-VM wrapper.
        Ok(unsafe { Value::from_raw(s.ctx(), wrapper.value().as_raw()) }.as_object().expect("wrapper"))
    }
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
            let page = page_of(s)?;
            let identity = page.event_identity(page.next_event_id());
            let reference = DataTransferRef { realm: s.realm().downgrade(), identity };
            s.wrap_new::<DataTransfer>(DataTransferData { store: Rc::new(store), reference }, nt, Some(identity))
        });
        def.accessor(
            "dropEffect",
            |s, d| Ok(s.string(&d.store.drop_effect.borrow())),
            |_s, d, v| {
                let v = dom_string(v)?;
                if matches!(v.as_str(), "none" | "copy" | "link" | "move") {
                    *d.store.drop_effect.borrow_mut() = v;
                    d.store.drop_effect_assigned.set(true);
                }
                Ok(())
            },
        );
        def.accessor(
            "effectAllowed",
            |s, d| Ok(s.string(&d.store.effect_allowed.borrow())),
            |_s, d, v| {
                let v = dom_string(v)?;
                if d.store.writable()
                    && matches!(
                        v.as_str(),
                        "none"
                            | "copy"
                            | "copyLink"
                            | "copyMove"
                            | "link"
                            | "linkMove"
                            | "move"
                            | "all"
                            | "uninitialized"
                    )
                {
                    *d.store.effect_allowed.borrow_mut() = v;
                }
                Ok(())
            },
        );
        def.getter("items", |s, d| {
            let wrapper = d.reference.get(s)?;
            if let Some(list) = s.get_hidden(wrapper, "__lpDataTransferItems")? {
                return Ok(list);
            }
            let list = s.wrap::<DataTransferItemList>(ItemListData { store: d.store.clone() }, None)?.value();
            // JS may add a back-reference to the owner. A traced edge keeps
            // stable identity while allowing the entire cycle to be collected.
            let root = Persistent::new(s, list);
            s.set_hidden(wrapper, "__lpDataTransferItems", root.get(s))?;
            Ok(list)
        });
        def.getter("types", |s, d| {
            let entries = d.store.entries.borrow();
            let listed = d.store.listed();
            let mut types: Vec<Value<'_>> = entries
                .iter()
                .filter(|e| listed && matches!(e.kind, Kind::Text { .. }))
                .map(|e| s.string(&e.ty))
                .collect();
            if listed && entries.iter().any(|e| matches!(e.kind, Kind::File { .. })) {
                types.push(s.string("Files"));
            }
            drop(entries);
            let array = s.array(&types)?;
            let freeze = s.global().get_atom("Object")?.as_object().map(|o| o.get_atom("freeze")).transpose()?;
            if let Some(f) = freeze.and_then(|f| f.as_object()) {
                f.call(None, &[array.value()])?;
            }
            Ok(array.value())
        });
        // The same FileList while the store's files stay the same.
        def.getter_this("files", |s, d, this| {
            let entries = d.store.entries.borrow();
            let readable = d.store.readable();
            let key: String = entries
                .iter()
                .filter(|e| readable && matches!(e.kind, Kind::File { .. }))
                .map(|e| format!("{},", e.id))
                .collect();
            if let (Some(list), Some(k)) = (s.get_hidden(this, FILES_LIST)?, s.get_hidden(this, FILES_KEY)?) {
                if k.as_string().is_some_and(|k| k == key) {
                    return Ok(list);
                }
            }
            let mut files = FileListBuilder::new(s)?;
            for e in entries.iter().filter(|_| readable) {
                if let Some(f) = d.store.file(s, e)? {
                    files.push(f.value())?;
                }
            }
            let list = files.finish(s)?;
            s.set_hidden(this, FILES_LIST, list)?;
            s.set_hidden(this, FILES_KEY, s.string(&key))?;
            Ok(list)
        });
        def.pure_method("getData", 1, |s, d, a| {
            let format = normalize_format(&arg_string(&a, 0, "DataTransfer.getData")?);
            if !d.store.readable() {
                return Ok(s.string(""));
            }
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
            if !d.store.writable() {
                return Ok(s.undefined());
            }
            d.store.entries.borrow_mut().retain(|e| !(matches!(e.kind, Kind::Text { .. }) && e.ty == format));
            d.store.push(format, Kind::Text { data });
            Ok(s.undefined())
        });
        def.method("clearData", 0, |s, d, a| {
            let format = if a.get(0).is_undefined() { None } else { Some(normalize_format(&dom_string(a.get(0))?)) };
            if !d.store.writable() {
                return Ok(s.undefined());
            }
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
        def.getter("length", |s, l| {
            Ok(s.number(if l.store.listed() { l.store.entries.borrow().len() as f64 } else { 0.0 }))
        });
        def.indexed(|s, l, i| {
            let id = l.store.entries.borrow().get(i as usize).filter(|_| l.store.listed()).map(|e| e.id);
            id.map(|id| item_value(s, &l.store, id)).transpose()
        });
        def.method("add", 1, |s, l, a| {
            a.require(1, "DataTransferItemList.add")?;
            if !l.store.writable() {
                return Ok(s.null());
            }
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
            if !l.store.writable() {
                return Err(JsError::dom("InvalidStateError", "The DataTransferItemList is not in read/write mode."));
            }
            let mut entries = l.store.entries.borrow_mut();
            if i < entries.len() {
                entries.remove(i);
            }
            Ok(s.undefined())
        });
        def.method("clear", 0, |s, l, _a| {
            if l.store.writable() {
                l.store.entries.borrow_mut().clear();
            }
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
        if !self.store.listed() {
            return None;
        }
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
        def.pure_method("getAsFile", 0, |s, it, _a| {
            let entries = it.store.entries.borrow();
            let file = match entries.iter().find(|e| e.id == it.id).filter(|_| it.store.readable()) {
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
            let (Some(Some(data)), true) = (data, callback.is_function() && it.store.readable()) else {
                return Ok(s.undefined());
            };
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

/// A natively created DataTransfer for a drag session. Its wrapper lives in
/// the page's main-world realm and is rooted while this
/// value exists; listeners in other worlds observe that same object, as with
/// a script-created DataTransfer carried by a DragEvent.
pub(crate) struct NativeTransfer {
    store: Rc<Store>,
    pub(crate) reference: DataTransferRef,
    _root: Persistent,
}

impl NativeTransfer {
    /// `items` are (format, data) string entries in order.
    pub(crate) fn new(
        page: &crate::page::PageDom,
        items: &[(String, String)],
        effect_allowed: &str,
        mode: Mode,
    ) -> Option<NativeTransfer> {
        let realm = page.main_world_realm()?;
        let s = realm.scope().ok()?;
        let store = Store::default();
        *store.drop_effect.borrow_mut() = "none".into();
        *store.effect_allowed.borrow_mut() = effect_allowed.into();
        for (format, data) in items {
            store.push(format.clone(), Kind::Text { data: data.clone() });
        }
        store.mode.set(mode);
        let store = Rc::new(store);
        let identity = page.event_identity(page.next_event_id());
        let reference = DataTransferRef { realm: s.realm().downgrade(), identity };
        let wrapper = s
            .wrap::<DataTransfer>(
                DataTransferData { store: store.clone(), reference: reference.clone() },
                Some(identity),
            )
            .ok()?;
        let root = Persistent::new(&s, wrapper.value());
        Some(NativeTransfer { store, reference, _root: root })
    }

    pub(crate) fn with_files(
        page: &crate::page::PageDom,
        items: &[(String, String)],
        files: &[crate::file::FileEntry],
        effect_allowed: &str,
        mode: Mode,
    ) -> Option<NativeTransfer> {
        let transfer = Self::new(page, items, effect_allowed, mode)?;
        for file in files {
            transfer.store.push(
                file.mime.to_ascii_lowercase(),
                Kind::File { bytes: file.bytes.clone(), name: file.name.clone(), last_modified: file.last_modified },
            );
        }
        Some(transfer)
    }

    pub(crate) fn set_mode(&self, mode: Mode) {
        self.store.mode.set(mode);
    }

    /// The (format, data) string entries in order.
    pub(crate) fn strings(&self) -> Vec<(String, String)> {
        self.store
            .entries
            .borrow()
            .iter()
            .filter_map(|e| match &e.kind {
                Kind::Text { data } => Some((e.ty.clone(), data.clone())),
                Kind::File { .. } => None,
            })
            .collect()
    }

    pub(crate) fn effect_allowed(&self) -> String {
        self.store.effect_allowed.borrow().clone()
    }

    pub(crate) fn drop_effect(&self) -> String {
        self.store.drop_effect.borrow().clone()
    }

    /// Whether a listener assigned a valid dropEffect to this transfer.
    pub(crate) fn drop_effect_assigned(&self) -> bool {
        self.store.drop_effect_assigned.get()
    }

    pub(crate) fn set_drop_effect(&self, effect: &str) {
        *self.store.drop_effect.borrow_mut() = effect.into();
    }
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<DataTransfer>();
    engine.register::<DataTransferItemList>();
    engine.register::<DataTransferItem>();
}
