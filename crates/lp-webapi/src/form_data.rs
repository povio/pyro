//! `FormData` (XHR §5): entry list of (name, string | File), constructible
//! from a `<form>` (+ submitter) via the form data set algorithm, and usable
//! as a fetch/XHR body (multipart/form-data).
//!
//! File values are kept as their JS `File` objects (rooted while the entry
//! exists) so `get()` returns the same object each time; Blob values become
//! Files named "blob" (or the given filename), as in browsers.
//! Iteration (`entries/keys/values/@@iterator/forEach`) snapshots the entry
//! list when the iterator is created (live mutation during iteration is not
//! reflected — documented deviation).

use std::cell::RefCell;

use bytes::Bytes;
use lp_js::{Args, ClassDef, JsClass, JsError, JsResult, NoParent, Persistent, Scope, Value};

use crate::file::{Blob, BlobData, File};
use crate::formdata::EntryValue;
use crate::util::*;

enum Entry {
    Text(String),
    File(Persistent),
}

#[derive(Default)]
pub struct FormDataData {
    entries: RefCell<Vec<(String, Entry)>>,
}

impl FormDataData {
    /// Entry list as plain values for multipart serialization.
    pub(crate) fn entry_values(&self, s: &Scope<'_>) -> Vec<(String, EntryValue)> {
        self.entries
            .borrow()
            .iter()
            .map(|(name, e)| {
                let v = match e {
                    Entry::Text(t) => EntryValue::Text(t.clone()),
                    Entry::File(p) => {
                        let file = s.unwrap::<Blob>(p.get(s));
                        let (bytes, mime, fname, lm) = match file.as_deref() {
                            Some(b) => (
                                b.bytes.clone(),
                                b.mime.clone(),
                                b.file.as_ref().map_or("blob".to_owned(), |f| f.0.clone()),
                                b.file.as_ref().map_or(0.0, |f| f.1),
                            ),
                            None => (Bytes::new(), String::new(), "blob".to_owned(), 0.0),
                        };
                        EntryValue::File(crate::file::FileEntry { name: fname, mime, bytes, last_modified: lm })
                    }
                };
                (name.clone(), v)
            })
            .collect()
    }
}

/// Converts `value` (+ optional filename) into an entry value.
fn entry_value(s: &Scope<'_>, value: Value<'_>, filename: Option<Value<'_>>) -> JsResult<Entry> {
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
                return Ok(Entry::File(Persistent::new(s, value)));
            }
            let name = filename.unwrap_or_else(|| file.as_ref().map_or("blob".to_owned(), |f| f.0.clone()));
            let lm = file.as_ref().map_or_else(crate::encoding::now_ms, |f| f.1);
            let f = s.wrap::<File>(BlobData { bytes, mime, file: Some((name, lm)) }, None)?;
            Ok(Entry::File(Persistent::new(s, f.value())))
        }
    }
}

fn entry_js<'s>(s: &Scope<'s>, e: &Entry) -> Value<'s> {
    match e {
        Entry::Text(t) => s.string(t),
        Entry::File(p) => p.get(s),
    }
}

fn name_arg(a: &Args<'_>, what: &str) -> JsResult<String> {
    arg_string(a, 0, what)
}

fn snapshot<'s>(s: &Scope<'s>, d: &FormDataData, kind: u8) -> JsResult<Value<'s>> {
    let mut items = lp_js::RootedVec::new();
    for (name, e) in d.entries.borrow().iter() {
        let v = match kind {
            0 => s.array(&[s.string(name), entry_js(s, e)])?.value(),
            1 => s.string(name),
            _ => entry_js(s, e),
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
            let data = FormDataData::default();
            let form = a.get(0);
            if !form.is_undefined() {
                let page = crate::target::page_of(s)?;
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
                let mut list = data.entries.borrow_mut();
                for (name, v) in entries {
                    let e = match v {
                        EntryValue::Text(t) => Entry::Text(t),
                        EntryValue::File(f) => {
                            let file = s.wrap::<File>(
                                BlobData { bytes: f.bytes, mime: f.mime, file: Some((f.name, f.last_modified)) },
                                None,
                            )?;
                            Entry::File(Persistent::new(s, file.value()))
                        }
                    };
                    list.push((name, e));
                }
            }
            s.wrap_new::<FormData>(data, nt, None)
        });
        def.method("append", 2, |s, d, a| {
            a.require(2, "FormData.append")?;
            let name = dom_string(a.get(0))?;
            let e = entry_value(s, a.get(1), a.opt(2))?;
            d.entries.borrow_mut().push((name, e));
            Ok(s.undefined())
        });
        def.method("set", 2, |s, d, a| {
            a.require(2, "FormData.set")?;
            let name = dom_string(a.get(0))?;
            let e = entry_value(s, a.get(1), a.opt(2))?;
            let mut list = d.entries.borrow_mut();
            match list.iter().position(|(n, _)| *n == name) {
                Some(first) => {
                    list[first].1 = e;
                    let mut i = 0;
                    list.retain(|(n, _)| {
                        let keep = *n != name || i == first;
                        i += 1;
                        keep
                    });
                }
                None => list.push((name, e)),
            }
            Ok(s.undefined())
        });
        def.method("delete", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.delete")?;
            d.entries.borrow_mut().retain(|(n, _)| *n != name);
            Ok(s.undefined())
        });
        def.method("get", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.get")?;
            Ok(d.entries.borrow().iter().find(|(n, _)| *n == name).map_or(s.null(), |(_, e)| entry_js(s, e)))
        });
        def.method("getAll", 1, |s, d, a| {
            let name = name_arg(&a, "FormData.getAll")?;
            let mut items = lp_js::RootedVec::new();
            for (_, e) in d.entries.borrow().iter().filter(|(n, _)| *n == name) {
                items.push(s, entry_js(s, e));
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

/// Multipart body for a FormData value: (bytes, Content-Type with boundary).
pub(crate) fn multipart_body(s: &Scope<'_>, v: Value<'_>) -> Option<(Bytes, String)> {
    let data = s.unwrap::<FormData>(v)?;
    let entries = data.entry_values(s);
    let (ct, body) = crate::formdata::multipart(&entries);
    Some((Bytes::from(body), ct))
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<FormData>();
}
