//! Blob, File and FileList (bytes held as refcounted `Bytes`).

use bytes::Bytes;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

use crate::node::NodeData;
use crate::util::*;

/// A selected file (CDP DOM.setFileInputFiles reads it from disk).
#[derive(Clone, Debug)]
pub struct FileEntry {
    pub name: String,
    pub mime: String,
    pub bytes: Bytes,
    /// Milliseconds since the Unix epoch.
    pub last_modified: f64,
}

pub struct BlobData {
    pub bytes: Bytes,
    pub mime: String,
    pub file: Option<(String, f64)>,
}

fn blob_parts(s: &Scope<'_>, parts: Value<'_>) -> JsResult<Vec<u8>> {
    let mut out = Vec::new();
    let Some(list) = parts.as_object() else {
        if parts.is_undefined() {
            return Ok(out);
        }
        return Err(JsError::Type("The provided value cannot be converted to a sequence.".into()));
    };
    let len = list.get_atom("length")?.to_number()? as u32;
    for i in 0..len {
        let p = list.get_index(i)?;
        if let Some(b) = s.unwrap::<Blob>(p) {
            out.extend_from_slice(&b.bytes);
        } else if let Some(bytes) = crate::encoding::buffer_source_bytes(s, p)? {
            out.extend_from_slice(&bytes);
        } else {
            out.extend_from_slice(dom_string(p)?.as_bytes());
        }
    }
    Ok(out)
}

fn mime_of(opts: Value<'_>) -> JsResult<String> {
    match opts.as_object() {
        Some(o) => {
            let t = o.get_atom("type")?;
            if t.is_undefined() {
                Ok(String::new())
            } else {
                let t = dom_string(t)?;
                Ok(if t.chars().all(|c| (' '..='~').contains(&c)) { t.to_ascii_lowercase() } else { String::new() })
            }
        }
        None => Ok(String::new()),
    }
}

pub struct Blob;
impl JsClass for Blob {
    const NAME: &'static str = "Blob";
    type Data = BlobData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, a| {
            let bytes = blob_parts(s, a.get(0))?;
            let mime = mime_of(a.get(1))?;
            s.wrap_new::<Blob>(BlobData { bytes: Bytes::from(bytes), mime, file: None }, nt, None)
        });
        def.getter("size", |s, b| Ok(s.number(b.bytes.len() as f64)));
        def.getter("type", |s, b| Ok(s.string(&b.mime)));
        def.method("slice", 0, |s, b, a| {
            let len = b.bytes.len() as i64;
            let clamp = |v: Value<'_>, default: i64| -> JsResult<usize> {
                if v.is_undefined() {
                    return Ok(default as usize);
                }
                let n = v.to_number()? as i64;
                Ok((if n < 0 { (len + n).max(0) } else { n.min(len) }) as usize)
            };
            let start = clamp(a.get(0), 0)?;
            let end = clamp(a.get(1), len)?.max(start);
            let mime = if a.get(2).is_undefined() { String::new() } else { dom_string(a.get(2))?.to_ascii_lowercase() };
            Ok(s.wrap::<Blob>(BlobData { bytes: b.bytes.slice(start..end), mime, file: None }, None)?.value())
        });
        def.method("text", 0, |s, b, _a| {
            let d = s.deferred()?;
            let text = String::from_utf8_lossy(&b.bytes).into_owned();
            d.resolve(s, s.string(&text))?;
            Ok(d.promise.get(s))
        });
        def.method("arrayBuffer", 0, |s, b, _a| {
            let d = s.deferred()?;
            let ab = crate::encoding::array_buffer(s, &b.bytes)?;
            d.resolve(s, ab)?;
            Ok(d.promise.get(s))
        });
        def.method("bytes", 0, |s, b, _a| {
            let d = s.deferred()?;
            let ab = crate::encoding::uint8_array(s, &b.bytes)?;
            d.resolve(s, ab)?;
            Ok(d.promise.get(s))
        });
        def.method("stream", 0, |s, b, _a| {
            let page = crate::target::page_of(s)?;
            let bytes = crate::encoding::uint8_array(s, &b.bytes)?;
            crate::streams::helper(s, &page, "fromBytes", &[bytes])
        });
    }
}

pub struct File;
impl JsClass for File {
    const NAME: &'static str = "File";
    type Data = BlobData;
    type Parent = Blob;
    fn define(def: &mut ClassDef<Self>) {
        def.constructor(2, |s, nt, a| {
            a.require(2, "File")?;
            let bytes = blob_parts(s, a.get(0))?;
            let name = dom_string(a.get(1))?;
            let mime = mime_of(a.get(2))?;
            let lm = match a.get(2).as_object() {
                Some(o) if !o.get_atom("lastModified")?.is_undefined() => o.get_atom("lastModified")?.to_number()?,
                _ => crate::encoding::now_ms(),
            };
            s.wrap_new::<File>(BlobData { bytes: Bytes::from(bytes), mime, file: Some((name, lm)) }, nt, None)
        });
        def.getter("name", |s, b| Ok(s.string(b.file.as_ref().map_or("", |f| f.0.as_str()))));
        def.getter("lastModified", |s, b| Ok(s.number(b.file.as_ref().map_or(0.0, |f| f.1))));
        def.getter("webkitRelativePath", |s, _b| Ok(s.string("")));
    }
}

/// Hidden slot on a `FileList` wrapper: the JS array of its `File` wrappers.
/// A traced edge, not a `Persistent`: a root held by the list's payload would
/// keep the files' realm (their prototypes reach the global) alive for as long
/// as the list's wrapper is reachable from that realm, and so forever once the
/// page closes.
const FILES: &str = "__lpFileListFiles";

pub struct FileListData {
    len: usize,
    wrapper: WrapperRef,
}

impl FileListData {
    /// The File at `index`, read from the hidden array of this list's own wrapper.
    fn get<'s>(&self, s: &Scope<'s>, index: usize) -> JsResult<Option<Value<'s>>> {
        if index >= self.len {
            return Ok(None);
        }
        let array = self.wrapper.hidden(s, FILES)?.and_then(|v| v.as_object()).ok_or(JsError::ContextDestroyed)?;
        Ok(Some(array.get_index(index as u32)?))
    }
}

/// Builds a `FileList`: the JS array exists first and every fresh `File`
/// wrapper goes into it immediately (a Rust `Vec<Value>` is not scanned by the
/// GC), then the array becomes the list wrapper's hidden slot.
pub(crate) struct FileListBuilder<'s> {
    array: Object<'s>,
    len: usize,
}

impl<'s> FileListBuilder<'s> {
    pub(crate) fn new(s: &Scope<'s>) -> JsResult<Self> {
        Ok(FileListBuilder { array: s.array(&[])?, len: 0 })
    }

    pub(crate) fn push(&mut self, file: Value<'_>) -> JsResult<()> {
        self.array.set_index(self.len as u32, file)?;
        self.len += 1;
        Ok(())
    }

    pub(crate) fn finish(self, s: &Scope<'s>) -> JsResult<Value<'s>> {
        let page = crate::target::page_of(s)?;
        let identity = page.event_identity(page.next_event_id());
        let data = FileListData { len: self.len, wrapper: WrapperRef::new(s, identity) };
        let list = s.wrap::<FileList>(data, Some(identity))?;
        s.set_hidden(list, FILES, self.array.value())?;
        Ok(list.value())
    }
}

pub struct FileList;
impl JsClass for FileList {
    const NAME: &'static str = "FileList";
    type Data = FileListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, l| Ok(s.number(l.len as f64)));
        def.method("item", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(l.get(s, i)?.unwrap_or(s.null()))
        });
        def.indexed(|s, l, i| l.get(s, i as usize));
        def.indexed_iterable();
    }
}

/// `input.files` (null for non-file inputs).
pub(crate) fn input_files<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    if n.page().dom.borrow().input_type(n.id).js()? != lp_dom::forms::InputType::File {
        return Ok(s.null());
    }
    let entries = n.page().files.borrow().get(&n.id).cloned().unwrap_or_default();
    let mut files = FileListBuilder::new(s)?;
    for e in entries {
        let f = s.wrap::<File>(
            BlobData { bytes: e.bytes.clone(), mime: e.mime.clone(), file: Some((e.name.clone(), e.last_modified)) },
            None,
        )?;
        files.push(f.value())?;
    }
    files.finish(s)
}

/// `input.files = list`: a FileList replaces the selection (bytes are shared,
/// not copied); null is ignored, as is any list on a non-file input. Events are
/// the caller's job (Playwright's in-memory setInputFiles dispatches them).
pub(crate) fn set_input_files(s: &Scope<'_>, n: &NodeData, v: Value<'_>) -> JsResult<()> {
    if v.is_nullish() {
        return Ok(());
    }
    let Some(list) = s.unwrap::<FileList>(v) else {
        return Err(JsError::Type(
            "Failed to set the 'files' property on 'HTMLInputElement': The provided value is not of type 'FileList'."
                .into(),
        ));
    };
    if n.page().dom.borrow().input_type(n.id).js()? != lp_dom::forms::InputType::File {
        return Ok(());
    }
    let mut entries = Vec::with_capacity(list.len);
    for i in 0..list.len {
        let f = list
            .get(s, i)?
            .and_then(|v| s.unwrap::<File>(v))
            .ok_or_else(|| JsError::Type("FileList entry is not a File".into()))?;
        let (name, last_modified) = f.file.clone().unwrap_or_default();
        entries.push(FileEntry { name, mime: f.mime.clone(), bytes: f.bytes.clone(), last_modified });
    }
    n.page().set_input_files(n.id, entries).js()
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Blob>();
    engine.register::<File>();
    engine.register::<FileList>();
}
