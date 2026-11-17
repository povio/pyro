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
//! Iteration (`entries/keys/values/@@iterator/forEach`) is live, as WebIDL's
//! iterable declarations require: each `next()` reads the entry at the
//! iterator's index at call time, so deleting during iteration skips, deleting
//! already-visited entries shifts the remainder, and appended entries are
//! reached. The iterator keeps its FormData alive through a hidden property
//! (`__lpFormDataTarget`) and resolves it again on every `next()`.
//!
//! No `RefCell` borrow of the entry list is ever held across a call that can reach
//! JS, and the hidden array has a null prototype, so script-defined indexed
//! accessors on `Array.prototype`/`Object.prototype` can neither observe nor change it.

use std::cell::{Cell, RefCell};

use bytes::Bytes;
use pyro_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::file::{Blob, BlobData, File};
use crate::formdata::EntryValue;
use crate::util::*;

/// The File objects of a FormData, a hidden property of its wrapper.
const FILES: &str = "__lpFormDataFiles";
/// The FormData an iterator walks, a hidden property of the iterator wrapper
/// (so the GC keeps the FormData, and its files array, alive with it).
const TARGET: &str = "__lpFormDataTarget";

/// `Date.now()`-style timestamp for a File created now: `lastModified` is an
/// integer number of milliseconds (WebIDL `long long`).
fn now_integer_ms() -> f64 {
    crate::encoding::now_ms().floor()
}

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
        None => {
            if filename.is_some_and(|f| !f.is_undefined()) {
                return Err(JsError::Type("parameter 2 is not of type 'Blob'.".into()));
            }
            Ok(Entry::Text(dom_string(value)?))
        }
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
            let lm = file.as_ref().map_or_else(now_integer_ms, |f| f.1);
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

/// HTML "submit button": `<input type=submit|image>` or a `<button>` in the
/// Submit Button state (`type=submit`, or Auto without `command`/`commandfor`).
fn is_submit_button(dom: &pyro_dom::Dom, id: pyro_dom::NodeId) -> bool {
    let Ok(e) = dom.element(id) else { return false };
    if !e.is_html() {
        return false;
    }
    match &**e.local_name() {
        "input" => matches!(
            dom.input_type(id),
            Ok(pyro_dom::forms::InputType::Submit | pyro_dom::forms::InputType::Image)
        ),
        "button" => dom.button_is_submit(id),
        _ => false,
    }
}

#[derive(Clone, Copy)]
enum IterKind {
    Entries,
    Keys,
    Values,
}

/// A live `FormData` iterator (WebIDL "iterable declaration" default
/// iterator object): `target` is resolved and `entries[index]` read on every
/// `next()`.
pub struct IteratorData {
    target: WrapperRef,
    index: Cell<usize>,
    done: Cell<bool>,
    kind: IterKind,
}

pub(crate) struct FormDataIterator;
impl JsClass for FormDataIterator {
    const NAME: &'static str = "FormData Iterator";
    type Data = IteratorData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        // No global name; `install` gives the prototype its iterator shape.
        def.not_exposed();
        // Argument 0 is the receiver, passed by the `next` shim `install` puts on
        // the prototype (native methods do not see `this`).
        def.method("next", 0, |s, it, a| {
            let value = if it.done.get() {
                s.undefined()
            } else {
                // The receiver's hidden TARGET property keeps the FormData alive and
                // finds it again without going through its (possibly closed) realm.
                let this = a.get(0).as_object().filter(|o| {
                    s.unwrap::<FormDataIterator>(o.value()).is_some_and(|u| std::ptr::eq::<IteratorData>(&*u, it))
                });
                let target = match this {
                    Some(o) => s.get_hidden(o, TARGET)?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)?,
                    None => it.target.get(s)?,
                };
                let d = s.unwrap::<FormData>(target.value()).ok_or(JsError::ContextDestroyed)?;
                let entry = d.entries.borrow().get(it.index.get()).cloned();
                match entry {
                    Some((name, e)) => {
                        it.index.set(it.index.get() + 1);
                        match it.kind {
                            IterKind::Keys => s.string(&name),
                            IterKind::Values => read_entry(s, &d, &mut None, &e)?,
                            IterKind::Entries => {
                                let mut pair = pyro_js::RootedVec::new();
                                pair.push(s, s.string(&name));
                                let value = read_entry(s, &d, &mut None, &e)?;
                                pair.push(s, value);
                                s.array(&pair.iter(s).collect::<Vec<_>>())?.value()
                            }
                        }
                    }
                    None => {
                        it.done.set(true);
                        s.undefined()
                    }
                }
            };
            let result = s.object();
            result.set_atom("value", value)?;
            result.set_atom("done", s.boolean(it.done.get()))?;
            Ok(result.value())
        });
    }
}

/// A new live iterator over `d`. `this` is the receiver as passed by the
/// `entries/keys/values` shims (`install`): it is `d`'s own wrapper, alive, and
/// is stored on the iterator; it is resolved through the identity map only when
/// a caller somehow reached the native without the shim.
fn iterator<'s>(s: &Scope<'s>, d: &FormDataData, this: Value<'s>, kind: IterKind) -> JsResult<Value<'s>> {
    let this = this
        .as_object()
        .filter(|o| s.unwrap::<FormData>(o.value()).is_some_and(|u| std::ptr::eq::<FormDataData>(&*u, d)));
    let target = match this {
        Some(o) => o,
        None => d.wrapper.get(s)?,
    };
    let it = s.wrap::<FormDataIterator>(
        IteratorData { target: d.wrapper.clone(), index: Cell::new(0), done: Cell::new(false), kind },
        None,
    )?;
    s.set_hidden(it, TARGET, target.value())?;
    Ok(it.value())
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
                        if !is_submit_button(&page.dom.borrow(), id) {
                            return Err(JsError::Type(
                                "Failed to construct 'FormData': The specified element is not a submit button.".into(),
                            ));
                        }
                        if page.dom.borrow().form_owner(id).ok().flatten() != Some(form_id) {
                            return Err(JsError::dom(
                                "NotFoundError",
                                "The specified element is not a submit button of this form.",
                            ));
                        }
                        Some(id)
                    }
                };
                match crate::formdata::construct_entry_list(Some(s), &page, form_id, submitter, "UTF-8") {
                    None => {
                        return Err(JsError::dom(
                            "InvalidStateError",
                            "Failed to construct 'FormData': The form is constructing its entry list.",
                        ))
                    }
                    Some(crate::formdata::EntryList::Entries(entries)) => fill(s, &data, &files, entries)?,
                    // The event's FormData, as its listeners left it; its
                    // Files keep their identity.
                    Some(crate::formdata::EntryList::FormData(fd)) => {
                        let src = s.unwrap::<FormData>(fd.value()).ok_or(JsError::ContextDestroyed)?;
                        let src_files = src.files(s)?;
                        let mut list = Vec::new();
                        for (name, e) in src.copy_entries() {
                            let e = match e {
                                Entry::Text(t) => Entry::Text(t),
                                Entry::File(slot) => Entry::File(data.store(&files, src_files.get_index(slot)?)?),
                            };
                            list.push((name, e));
                        }
                        *data.entries.borrow_mut() = list;
                    }
                }
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
        def.pure_method("get", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.get")?;
            let found = d.entries.borrow().iter().find(|(n, _)| *n == name).map(|(_, e)| e.clone());
            match found {
                None => Ok(s.null()),
                Some(e) => read_entry(s, d, &mut None, &e),
            }
        });
        def.pure_method("getAll", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.getAll")?;
            let matching: Vec<Entry> =
                d.entries.borrow().iter().filter(|(n, _)| *n == name).map(|(_, e)| e.clone()).collect();
            let mut files = None;
            let mut items = pyro_js::RootedVec::new();
            for e in &matching {
                let v = read_entry(s, d, &mut files, e)?;
                items.push(s, v);
            }
            Ok(s.array(&items.iter(s).collect::<Vec<_>>())?.value())
        });
        def.pure_method("has", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.has")?;
            Ok(s.boolean(d.entries.borrow().iter().any(|(n, _)| *n == name)))
        });
        // Argument 0 is the receiver, passed by the shims `install` puts on the
        // prototype (native methods do not see `this`).
        def.method("entries", 0, |s, d, a| iterator(s, d, a.get(0), IterKind::Entries));
        def.method("keys", 0, |s, d, a| iterator(s, d, a.get(0), IterKind::Keys));
        def.method("values", 0, |s, d, a| iterator(s, d, a.get(0), IterKind::Values));
    }
}

/// Stores `entries` in `data`, File entries as new File objects in `files`.
fn fill(s: &Scope<'_>, data: &FormDataData, files: &Object<'_>, entries: Vec<(String, EntryValue)>) -> JsResult<()> {
    let mut list = Vec::with_capacity(entries.len());
    for (name, v) in entries {
        let e = match v {
            EntryValue::Text(t) => Entry::Text(t),
            EntryValue::File(f) => {
                let file = s.wrap::<File>(
                    BlobData { bytes: f.bytes, mime: f.mime, file: Some((f.name, f.last_modified)) },
                    None,
                )?;
                Entry::File(data.store(files, file.value())?)
            }
        };
        list.push((name, e));
    }
    *data.entries.borrow_mut() = list;
    Ok(())
}

/// A new FormData of `s`'s realm holding `entries`.
pub(crate) fn with_entries<'s>(s: &Scope<'s>, entries: Vec<(String, EntryValue)>) -> JsResult<Object<'s>> {
    let page = crate::target::page_of(s)?;
    let identity = page.event_identity(page.next_event_id());
    let data = FormDataData::new(WrapperRef::new(s, identity));
    let files = s.array(&[])?;
    files.set_prototype(s.null());
    let fd = s.wrap::<FormData>(data, Some(identity))?;
    s.set_hidden(fd, FILES, files.value())?;
    let d = s.unwrap::<FormData>(fd.value()).expect("fresh FormData");
    fill(s, &d, &files, entries)?;
    Ok(fd)
}

/// The entries of the FormData `fd`.
pub(crate) fn entries_of(s: &Scope<'_>, fd: Object<'_>) -> JsResult<Vec<(String, EntryValue)>> {
    let d = s.unwrap::<FormData>(fd.value()).ok_or(JsError::ContextDestroyed)?;
    d.entry_values(s, fd.value())
}

/// A new FormData holding a parsed `application/x-www-form-urlencoded` body.
pub(crate) fn from_urlencoded<'s>(s: &Scope<'s>, bytes: &[u8]) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let identity = page.event_identity(page.next_event_id());
    let data = FormDataData::new(WrapperRef::new(s, identity));
    *data.entries.borrow_mut() =
        url::form_urlencoded::parse(bytes).map(|(n, v)| (n.into_owned(), Entry::Text(v.into_owned()))).collect();
    let files = s.array(&[])?;
    files.set_prototype(s.null());
    let fd = s.wrap::<FormData>(data, Some(identity))?;
    s.set_hidden(fd, FILES, files.value())?;
    Ok(fd.value())
}

/// `Body.formData()` for a `multipart/form-data` body: each part is a text
/// entry, or a File when it has a `filename`.
pub(crate) fn from_multipart<'s>(s: &Scope<'s>, bytes: &[u8], boundary: &str) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let identity = page.event_identity(page.next_event_id());
    let data = FormDataData::new(WrapperRef::new(s, identity));
    let files = s.array(&[])?;
    files.set_prototype(s.null());
    let fd = s.wrap::<FormData>(data, Some(identity))?;
    s.set_hidden(fd, FILES, files.value())?;
    let d = s.unwrap::<FormData>(fd.value()).expect("fresh FormData");
    let parts = parse_multipart(bytes, boundary)
        .ok_or_else(|| JsError::Type("Failed to parse the multipart/form-data body".into()))?;
    for part in parts {
        let entry = match part.filename {
            None => Entry::Text(String::from_utf8_lossy(&part.body).into_owned()),
            Some(filename) => {
                let file = s.wrap::<crate::file::File>(
                    crate::file::BlobData {
                        bytes: Bytes::from(part.body),
                        mime: part.mime.unwrap_or_else(|| "text/plain".into()),
                        file: Some((filename, now_integer_ms())),
                    },
                    None,
                )?;
                Entry::File(d.store(&files, file.value())?)
            }
        };
        d.entries.borrow_mut().push((part.name, entry));
    }
    Ok(fd.value())
}

struct MultipartPart {
    name: String,
    filename: Option<String>,
    mime: Option<String>,
    body: Vec<u8>,
}

/// Splits a multipart/form-data body at `boundary`; `None` on malformed input.
fn parse_multipart(bytes: &[u8], boundary: &str) -> Option<Vec<MultipartPart>> {
    let delimiter = format!("--{boundary}").into_bytes();
    let find = |hay: &[u8], needle: &[u8], from: usize| -> Option<usize> {
        hay[from..].windows(needle.len()).position(|w| w == needle).map(|p| p + from)
    };
    let mut parts = Vec::new();
    let mut pos = find(bytes, &delimiter, 0)? + delimiter.len();
    loop {
        if bytes[pos..].starts_with(b"--") {
            return Some(parts);
        }
        // CRLF after the delimiter, then headers up to a blank line.
        let line_end = find(bytes, b"\r\n", pos)?;
        pos = line_end + 2;
        let headers_end = find(bytes, b"\r\n\r\n", pos)?;
        let headers = String::from_utf8_lossy(&bytes[pos..headers_end]).into_owned();
        pos = headers_end + 4;
        let mut terminator = b"\r\n".to_vec();
        terminator.extend_from_slice(&delimiter);
        let body_end = find(bytes, &terminator, pos)?;
        let body = bytes[pos..body_end].to_vec();
        pos = body_end + terminator.len();
        let mut name = None;
        let mut filename = None;
        let mut mime = None;
        for line in headers.split("\r\n") {
            let Some((key, value)) = line.split_once(':') else { continue };
            let value = value.trim();
            if key.eq_ignore_ascii_case("content-disposition") {
                for param in value.split(';').skip(1) {
                    let Some((k, v)) = param.trim().split_once('=') else { continue };
                    let v = v.trim().trim_matches('"').replace("%22", "\"").replace("%0D", "\r").replace("%0A", "\n");
                    match k.trim() {
                        "name" => name = Some(v),
                        "filename" => filename = Some(v),
                        _ => {}
                    }
                }
            } else if key.eq_ignore_ascii_case("content-type") {
                mime = Some(value.to_owned());
            }
        }
        parts.push(MultipartPart { name: name?, filename, mime, body });
    }
}

/// Installs `@@iterator` and `forEach` on `FormData.prototype` and shapes the
/// iterator prototype like a WebIDL default iterator object (inherits
/// `%IteratorPrototype%`, `@@toStringTag` "FormData Iterator", no constructor
/// and no global). `hooks` is install.js `formData`: the steps completing
/// FormData.prototype and the iterator prototype, run once their members are
/// installed (`after_populate`).
pub(crate) fn install(s: &Scope<'_>, hooks: pyro_js::Value<'_>) -> JsResult<()> {
    let hooks = hooks.as_object().expect("FormData hooks");
    let hook = |i: u32| hooks.get_index(i).map(|v| v.as_object().expect("FormData hook"));
    s.after_populate::<FormData>(hook(0)?)?;
    s.after_populate::<FormDataIterator>(hook(1)?)
}

/// Multipart body for a FormData value: (bytes, Content-Type with boundary), or
/// `None` when `v` is not a FormData. Fails (instead of sending an empty file)
/// when an entry's File can no longer be read.
pub(crate) fn multipart_body(s: &Scope<'_>, v: Value<'_>) -> JsResult<Option<(Bytes, String)>> {
    let Some(data) = s.unwrap::<FormData>(v) else { return Ok(None) };
    let entries = data.entry_values(s, v)?;
    let (ct, body) = crate::formdata::multipart(&entries, encoding_rs::UTF_8);
    Ok(Some((Bytes::from(body), ct)))
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<FormData>();
    engine.register::<FormDataIterator>();
}
