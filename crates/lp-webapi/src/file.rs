//! Blob, File and FileList (bytes held as refcounted `Bytes`).

use bytes::Bytes;
use lp_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Scope, Value};

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
    let len = list.get("length")?.to_number()? as u32;
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
            let t = o.get("type")?;
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
                Some(o) if !o.get("lastModified")?.is_undefined() => o.get("lastModified")?.to_number()?,
                _ => crate::encoding::now_ms(),
            };
            s.wrap_new::<File>(BlobData { bytes: Bytes::from(bytes), mime, file: Some((name, lm)) }, nt, None)
        });
        def.getter("name", |s, b| Ok(s.string(b.file.as_ref().map_or("", |f| f.0.as_str()))));
        def.getter("lastModified", |s, b| Ok(s.number(b.file.as_ref().map_or(0.0, |f| f.1))));
        def.getter("webkitRelativePath", |s, _b| Ok(s.string("")));
    }
}

pub struct FileListData {
    files: Vec<lp_js::Persistent>,
}

pub struct FileList;
impl JsClass for FileList {
    const NAME: &'static str = "FileList";
    type Data = FileListData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.getter("length", |s, l| Ok(s.number(l.files.len() as f64)));
        def.method("item", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(l.files.get(i).map_or(s.null(), |f| f.get(s)))
        });
        def.indexed(|s, l, i| Ok(l.files.get(i as usize).map(|f| f.get(s))));
        def.indexed_iterable();
    }
}

/// `input.files` (null for non-file inputs).
pub(crate) fn input_files<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    if n.page().dom.borrow().input_type(n.id).js()? != lp_dom::forms::InputType::File {
        return Ok(s.null());
    }
    let entries = n.page().files.borrow().get(&n.id).cloned().unwrap_or_default();
    let mut files = Vec::with_capacity(entries.len());
    for e in entries {
        let f = s.wrap::<File>(
            BlobData { bytes: e.bytes.clone(), mime: e.mime.clone(), file: Some((e.name.clone(), e.last_modified)) },
            None,
        )?;
        files.push(lp_js::Persistent::new(s, f.value()));
    }
    Ok(s.wrap::<FileList>(FileListData { files }, None)?.value())
}

pub(crate) fn register(engine: &lp_js::Engine) {
    engine.register::<Blob>();
    engine.register::<File>();
    engine.register::<FileList>();
}
