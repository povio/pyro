//! `FormData` (XHR §5): entry list of (name, string | File), constructible
//! from a `<form>` (+ submitter) via the form data set algorithm, and usable
//! as a fetch/XHR body (multipart/form-data).
//!
//! File values are kept as their JS `File` objects in a JS array that is a hidden
//! property of the FormData wrapper (`__lpFormDataFiles`): the GC traces it, so the
//! Files live exactly as long as the FormData and `get()` returns the same object
//! each time, and no payload-owned `Persistent` can keep a closed realm alive. An
//! entry stores only its slot number in that array. Slots are never reused; removed
//! entries leave an `undefined` tombstone so a dropped File becomes collectable.
//! Blob values become Files named "blob" (or the given filename), as in browsers.
//! Iteration (`entries/keys/values/@@iterator/forEach`) snapshots the entry
//! list when the iterator is created (live mutation during iteration is not
//! reflected — documented deviation).
//!
//! No `RefCell` borrow of the entry list is ever held across a call that can reach
//! JS, and the hidden array has a null prototype, so script-defined indexed
//! accessors on `Array.prototype`/`Object.prototype` can neither observe nor change it.

use std::cell::{Cell, RefCell};

use bytes::Bytes;
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::file::{Blob, BlobData, File};
use crate::formdata::EntryValue;
use crate::util::*;

/// The File objects of a FormData, a hidden property of its wrapper.
const FILES: &str = "__lpFormDataFiles";

#[derive(Clone)]
enum Entry {
    Text(String),
    /// Slot number in the hidden files array.
    File(u32),
}

pub struct FormDataData {
    entries: RefCell<Vec<(String, Entry)>>,
    wrapper: WrapperRef,
    next_slot: Cell<u32>,
}

impl FormDataData {
    fn new(wrapper: WrapperRef) -> Self {
        FormDataData { entries: RefCell::new(Vec::new()), wrapper, next_slot: Cell::new(0) }
    }

    /// The hidden files array of this FormData's own wrapper. Fails with
    /// `ContextDestroyed` once the realm that created the FormData is disposed.
    fn files<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        self.wrapper.hidden(s, FILES)?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)
    }

    /// Puts `file` in the next free slot of `files`. A fresh File wrapper goes into
    /// the array immediately; a Rust `Vec<Value>` is not scanned by the GC.
    fn store(&self, files: &Object<'_>, file: Value<'_>) -> JsResult<u32> {
        let slot = self.next_slot.get();
        let next = slot.checked_add(1).ok_or_else(|| JsError::Type("Too many files in this FormData".into()))?;
        files.set_index(slot, file)?;
        self.next_slot.set(next);
        Ok(slot)
    }

    /// A copy of the entry list, so no borrow is held while JS runs.
    fn copy_entries(&self) -> Vec<(String, Entry)> {
        self.entries.borrow().clone()
    }

    /// Releases removed Files (best effort: the entries are already gone from the
    /// list, and a failed write only delays the release until the FormData dies).
    fn release(&self, s: &Scope<'_>, slots: &[u32]) {
        if slots.is_empty() {
            return;
        }
        if let Ok(files) = self.files(s) {
            for slot in slots {
                let _ = files.set_index(*slot, s.undefined());
            }
        }
    }

    /// Entry list as plain values for multipart serialization. `this` is the
    /// FormData value being serialized: its hidden array is read directly, so
    /// serialization does not depend on the owner realm being alive and never
    /// silently substitutes an empty file for an unreadable one.
    pub(crate) fn entry_values<'s>(&self, s: &Scope<'s>, this: Value<'_>) -> JsResult<Vec<(String, EntryValue)>> {
        let entries = self.copy_entries();
        let mut files: Option<Object<'s>> = None;
        let mut out = Vec::with_capacity(entries.len());
        for (name, e) in entries {
            let v = match e {
                Entry::Text(t) => EntryValue::Text(t),
                Entry::File(slot) => {
                    let array = match &files {
                        Some(a) => *a,
                        None => {
                            let owner = this.as_object().ok_or(JsError::ContextDestroyed)?;
                            let a = s
                                .get_hidden(owner, FILES)?
                                .and_then(|v| v.as_object())
                                .ok_or(JsError::ContextDestroyed)?;
                            files = Some(a);
                            a
                        }
                    };
                    let file = array.get_index(slot)?;
                    let blob = s
                        .unwrap::<Blob>(file)
                        .ok_or_else(|| JsError::Type("A FormData file entry is no longer available".into()))?;
                    EntryValue::File(crate::file::FileEntry {
                        name: blob.file.as_ref().map_or("blob".to_owned(), |f| f.0.clone()),
                        mime: blob.mime.clone(),
                        bytes: blob.bytes.clone(),
                        last_modified: blob.file.as_ref().map_or(0.0, |f| f.1),
                    })
                }
            };
            out.push((name, v));
        }
        Ok(out)
    }
}

/// Converts `value` (+ optional filename) into an entry value. A File goes into the
/// next slot of the hidden array right here (before the caller touches the entry
/// list), so no fresh wrapper is ever held only by Rust.
fn entry_value(s: &Scope<'_>, d: &FormDataData, value: Value<'_>, filename: Option<Value<'_>>) -> JsResult<Entry> {
    let blob = s.unwrap::<Blob>(value).map(|b| (b.bytes.clone(), b.mime.clone(), b.file.clone()));
    match blob {
        None => Ok(Entry::Text(dom_string(value)?)),
        Some((bytes, mime, file)) => {
            let filename = match filename.filter(|f| !f.is_undefined()) {
                Some(f) => Some(dom_string(f)?),
                None => None,
            };
            // Files keep identity unless renamed; Blobs become Files.
            if file.is_some() && filename.is_none() {
                return Ok(Entry::File(d.store(&d.files(s)?, value)?));
            }
            let name = filename.unwrap_or_else(|| file.as_ref().map_or("blob".to_owned(), |f| f.0.clone()));
            let lm = file.as_ref().map_or_else(crate::encoding::now_ms, |f| f.1);
            let files = d.files(s)?;
            let f = s.wrap::<File>(BlobData { bytes, mime, file: Some((name, lm)) }, None)?;
            Ok(Entry::File(d.store(&files, f.value())?))
        }
    }
}

/// The JS value of an entry. `files` caches the hidden array for the caller's loop.
fn read_entry<'s>(s: &Scope<'s>, d: &FormDataData, files: &mut Option<Object<'s>>, e: &Entry) -> JsResult<Value<'s>> {
    match e {
        Entry::Text(t) => Ok(s.string(t)),
        Entry::File(slot) => {
            if files.is_none() {
                *files = Some(d.files(s)?);
            }
            files.as_ref().expect("files set above").get_index(*slot)
        }
    }
}

fn name_arg(a: &Args<'_>, what: &str) -> JsResult<String> {
    arg_string(a, 0, what)
}

fn snapshot<'s>(s: &Scope<'s>, d: &FormDataData, kind: u8) -> JsResult<Value<'s>> {
    let entries = d.copy_entries();
    let mut files = None;
    let mut items = lp_js::RootedVec::new();
    for (name, e) in &entries {
        let v = match kind {
            0 => {
                let value = read_entry(s, d, &mut files, e)?;
                s.array(&[s.string(name), value])?.value()
            }
            1 => s.string(name),
            _ => read_entry(s, d, &mut files, e)?,
        };
        items.push(s, v);
    }
    let arr = s.array(&items.iter(s).collect::<Vec<_>>())?;
    let values = arr.get("values")?.as_object().ok_or_else(|| JsError::Type("Array.prototype.values".into()))?;
    values.call(Some(arr), &[])
}

pub struct FormData;
impl JsClass for FormData {
    const NAME: &'static str = "FormData";
    type Data = FormDataData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            // The wrapper needs an identity (the payload finds its own wrapper again
            // through the realm's identity map) and the hidden array must exist
            // before any File wrapper is created.
            let page = crate::target::page_of(s)?;
            let identity = page.event_identity(page.next_event_id());
            let data = FormDataData::new(WrapperRef::new(s, identity));
            let files = s.array(&[])?;
            files.set_prototype(s.null());
            let form = a.get(0);
            if !form.is_undefined() {
                let node = s.unwrap::<crate::node::Node>(form).map(|n| n.id);
                let is_form = node.is_some_and(|id| {
                    page.dom.borrow().element(id).is_ok_and(|e| e.is_html() && *e.local_name() == *"form")
                });
                let Some(form_id) = node.filter(|_| is_form) else {
                    return Err(JsError::Type(
                        "Failed to construct 'FormData': parameter 1 is not of type 'HTMLFormElement'.".into(),
                    ));
                };
                let submitter = match a.get(1) {
                    v if v.is_nullish() => None,
                    v => {
                        let id = s.unwrap::<crate::node::Node>(v).map(|n| n.id).ok_or_else(|| {
                            JsError::Type("Failed to construct 'FormData': submitter is not an HTMLElement.".into())
                        })?;
                        if page.dom.borrow().form_owner(id).ok().flatten() != Some(form_id) {
                            return Err(JsError::dom(
                                "NotFoundError",
                                "The specified element is not a submit button of this form.",
                            ));
                        }
                        Some(id)
                    }
                };
                let entries = crate::formdata::form_entries(&page, form_id, submitter);
                let mut list = Vec::with_capacity(entries.len());
                for (name, v) in entries {
                    let e = match v {
                        EntryValue::Text(t) => Entry::Text(t),
                        EntryValue::File(f) => {
                            let file = s.wrap::<File>(
                                BlobData { bytes: f.bytes, mime: f.mime, file: Some((f.name, f.last_modified)) },
                                None,
                            )?;
                            Entry::File(data.store(&files, file.value())?)
                        }
                    };
                    list.push((name, e));
                }
                *data.entries.borrow_mut() = list;
            }
            let fd = s.wrap_new::<FormData>(data, nt, Some(identity))?;
            s.set_hidden(fd, FILES, files.value())?;
            Ok(fd)
        });
        def.method("append", 2, |s, d, a| {
            a.require(2, "FormData.append")?;
            let name = dom_string(a.get(0))?;
            let e = entry_value(s, d, a.get(1), a.opt(2))?;
            d.entries.borrow_mut().push((name, e));
            Ok(s.undefined())
        });
        def.method("set", 2, |s, d, a| {
            a.require(2, "FormData.set")?;
            let name = dom_string(a.get(0))?;
            // The new entry (and its slot) exists before the list is borrowed.
            let e = entry_value(s, d, a.get(1), a.opt(2))?;
            let mut removed = Vec::new();
            {
                let mut list = d.entries.borrow_mut();
                match list.iter().position(|(n, _)| *n == name) {
                    Some(first) => {
                        if let Entry::File(slot) = std::mem::replace(&mut list[first].1, e) {
                            removed.push(slot);
                        }
                        let mut i = 0;
                        list.retain(|(n, old)| {
                            let keep = *n != name || i == first;
                            if !keep {
                                if let Entry::File(slot) = old {
                                    removed.push(*slot);
                                }
                            }
                            i += 1;
                            keep
                        });
                    }
                    None => list.push((name, e)),
                }
            }
            d.release(s, &removed);
            Ok(s.undefined())
        });
        def.method("delete", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.delete")?;
            let mut removed = Vec::new();
            d.entries.borrow_mut().retain(|(n, e)| {
                if *n != name {
                    return true;
                }
                if let Entry::File(slot) = e {
                    removed.push(*slot);
                }
                false
            });
            d.release(s, &removed);
            Ok(s.undefined())
        });
        def.method("get", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.get")?;
            let found = d.entries.borrow().iter().find(|(n, _)| *n == name).map(|(_, e)| e.clone());
            match found {
                None => Ok(s.null()),
                Some(e) => read_entry(s, d, &mut None, &e),
            }
        });
        def.method("getAll", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.getAll")?;
            let matching: Vec<Entry> =
                d.entries.borrow().iter().filter(|(n, _)| *n == name).map(|(_, e)| e.clone()).collect();
            let mut files = None;
            let mut items = lp_js::RootedVec::new();
            for e in &matching {
                let v = read_entry(s, d, &mut files, e)?;
                items.push(s, v);
            }
            Ok(s.array(&items.iter(s).collect::<Vec<_>>())?.value())
        });
        def.method("has", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.has")?;
            Ok(s.boolean(d.entries.borrow().iter().any(|(n, _)| *n == name)))
        });
        def.method("entries", 0, |s, d, _a| snapshot(s, d, 0));
        def.method("keys", 0, |s, d, _a| snapshot(s, d, 1));
        def.method("values", 0, |s, d, _a| snapshot(s, d, 2));
    }
}

/// Installs `@@iterator` and `forEach` on `FormData.prototype`.
pub(crate) fn install(s: &Scope<'_>) -> JsResult<()> {
    s.eval(
        r#"(() => {
        const proto = FormData.prototype, apply = Reflect.apply, entries = proto.entries;
        Object.defineProperty(proto, Symbol.iterator, { value: entries, writable: true, configurable: true });
        Object.defineProperty(proto, 'forEach', { value: function (callback, thisArg) {
            if (typeof callback !== 'function') throw new TypeError('callback must be callable');
            for (const pair of apply(entries, this, [])) apply(callback, thisArg, [pair[1], pair[0], this]);
        }, writable: true, enumerable: true, configurable: true });
    })()"#,
        Some("lightningpanda://internal/form-data.js"),
        1,
    )?;
    Ok(())
}

/// Multipart body for a FormData value: (bytes, Content-Type with boundary), or
/// `None` when `v` is not a FormData. Fails (instead of sending an empty file)
/// when an entry's File can no longer be read.
pub(crate) fn multipart_body(s: &Scope<'_>, v: Value<'_>) -> JsResult<Option<(Bytes, String)>> {
    let Some(data) = s.unwrap::<FormData>(v) else { return Ok(None) };
    let entries = data.entry_values(s, v)?;
    let (ct, body) = crate::formdata::multipart(&entries);
    Ok(Some((Bytes::from(body), ct)))
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<FormData>();
}
