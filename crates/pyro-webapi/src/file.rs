//! Blob, File and FileList (bytes held as refcounted `Bytes`).

use bytes::Bytes;
use pyro_js::{ClassDef, JsClass, JsError, JsResult, NoParent, Object, Scope, Value};

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

/// Keeps install.js's `blobParts` helper for the Blob and File constructors.
pub(crate) fn install(s: &Scope<'_>, helper: Value<'_>) -> JsResult<()> {
    crate::target::page_of(s)?.blob_parts.borrow_mut().insert(s.realm_id(), pyro_js::Persistent::new(s, helper));
    Ok(())
}

/// `endings` of BlobPropertyBag: whether line endings become native (`\n`).
fn native_endings(opts: Value<'_>) -> JsResult<bool> {
    let Some(o) = opts.as_object() else { return Ok(false) };
    let e = o.get_atom("endings")?;
    if e.is_undefined() {
        return Ok(false);
    }
    match dom_string(e)?.as_str() {
        "transparent" => Ok(false),
        "native" => Ok(true),
        other => Err(JsError::Type(
            format!("Failed to read the 'endings' property from 'BlobPropertyBag': The provided value '{other}' is not a valid enum value of type EndingType.").into(),
        )),
    }
}

/// A BlobPropertyBag argument: undefined, null or an object.
fn check_bag(what: &str, opts: Value<'_>) -> JsResult<()> {
    if opts.is_undefined() || opts.is_null() || opts.as_object().is_some() {
        Ok(())
    } else {
        Err(JsError::Type(format!("Failed to construct '{what}': The provided value is not of type 'BlobPropertyBag'.").into()))
    }
}

/// "Convert line endings to native" (\n here): CR LF and lone CR become LF.
fn native_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

/// `blobParts` converted as `sequence<BlobPart>` (install.js), before the
/// options are read.
fn convert_parts<'s>(s: &Scope<'s>, parts: Value<'s>) -> JsResult<Option<Object<'s>>> {
    let page = crate::target::page_of(s)?;
    let convert = page.blob_parts.borrow().get(&s.realm_id()).and_then(|p| p.get(s).as_object());
    let Some(convert) = convert else { return Ok(None) };
    Ok(convert.call(None, &[parts])?.as_object())
}

/// The bytes of converted `blobParts` (File API "process blob parts").
fn blob_parts(s: &Scope<'_>, list: Option<Object<'_>>, native: bool) -> JsResult<Vec<u8>> {
    let mut out = Vec::new();
    let Some(list) = list else { return Ok(out) };
    let len = list.get_atom("length")?.to_number()? as u32;
    for i in 0..len {
        let p = list.get_index(i)?;
        if let Some(b) = s.unwrap::<Blob>(p) {
            out.extend_from_slice(&b.bytes);
        } else if p.as_object().is_some() {
            if let Some(bytes) = crate::encoding::buffer_source_bytes(s, p)? {
                out.extend_from_slice(&bytes);
            }
        } else {
            let text = dom_string(p)?;
            out.extend_from_slice(if native { native_newlines(&text) } else { text }.as_bytes());
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
            // Web IDL converts the arguments in order, the dictionary's members by name.
            let parts = convert_parts(s, a.get(0))?;
            check_bag("Blob", a.get(1))?;
            let native = native_endings(a.get(1))?;
            let mime = mime_of(a.get(1))?;
            let bytes = blob_parts(s, parts, native)?;
            s.wrap_new::<Blob>(BlobData { bytes: Bytes::from(bytes), mime, file: None }, nt, None)
        });
        def.getter("size", |s, b| Ok(s.number(b.bytes.len() as f64)));
        def.getter("type", |s, b| Ok(s.string(&b.mime)));
        def.pure_method("slice", 0, |s, b, a| {
            let len = b.bytes.len() as i64;
            // [Clamp] long long: NaN is 0, halves round to even.
            let clamp = |v: Value<'_>, default: i64| -> JsResult<usize> {
                if v.is_undefined() {
                    return Ok(default as usize);
                }
                let x = v.to_number()?;
                let n = if x.is_nan() { 0 } else { x.round_ties_even().clamp(-(2f64.powi(53) - 1.0), 2f64.powi(53) - 1.0) as i64 };
                Ok((if n < 0 { (len + n).max(0) } else { n.min(len) }) as usize)
            };
            let start = clamp(a.get(0), 0)?;
            let end = clamp(a.get(1), len)?.max(start);
            // A content type outside U+0020..U+007E is the empty string.
            let mime = if a.get(2).is_undefined() {
                String::new()
            } else {
                let t = dom_string(a.get(2))?;
                if t.chars().all(|c| (' '..='~').contains(&c)) { t.to_ascii_lowercase() } else { String::new() }
            };
            Ok(s.wrap::<Blob>(BlobData { bytes: b.bytes.slice(start..end), mime, file: None }, None)?.value())
        });
        def.promise_method("text", 0, |s, b, _a| {
            let d = s.deferred()?;
            let text = String::from_utf8_lossy(&b.bytes).into_owned();
            d.resolve(s, s.string(&text))?;
            Ok(d.promise.get(s))
        });
        def.promise_method("arrayBuffer", 0, |s, b, _a| {
            let d = s.deferred()?;
            let ab = crate::encoding::array_buffer(s, &b.bytes)?;
            d.resolve(s, ab)?;
            Ok(d.promise.get(s))
        });
        def.promise_method("bytes", 0, |s, b, _a| {
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
        def.method("textStream", 0, |s, b, _a| {
            let page = crate::target::page_of(s)?;
            let bytes = crate::encoding::uint8_array(s, &b.bytes)?;
            crate::streams::helper(s, &page, "textFromBytes", &[bytes])
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
            let parts = convert_parts(s, a.get(0))?;
            let name = dom_string(a.get(1))?;
            check_bag("File", a.get(2))?;
            let native = native_endings(a.get(2))?;
            let mime = mime_of(a.get(2))?;
            let bytes = blob_parts(s, parts, native)?;
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
        def.pure_method("item", 1, |s, l, a| {
            let i = to_u32(a.get(0))? as usize;
            Ok(l.get(s, i)?.unwrap_or(s.null()))
        });
        def.indexed(|s, l, i| l.get(s, i as usize));
        def.indexed_iterable();
    }
}

/// A new wrapper in `s`'s realm over the same bytes (Blob and File are
/// immutable): a File stays a File, any subclass becomes its base class.
fn wrap_blob<'s>(s: &Scope<'s>, b: &BlobData) -> JsResult<Object<'s>> {
    let data = BlobData { bytes: b.bytes.clone(), mime: b.mime.clone(), file: b.file.clone() };
    if b.file.is_some() {
        s.wrap::<File>(data, None)
    } else {
        s.wrap::<Blob>(data, None)
    }
}

/// Structured clone of a serializable host object (Blob, File, FileList)
/// into `s`'s realm; `None` for anything else. The bytes are shared.
pub(crate) fn clone_host<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Option<Value<'s>>> {
    if let Some(b) = s.unwrap::<Blob>(value) {
        return Ok(Some(wrap_blob(s, &b)?.value()));
    }
    let Some(list) = s.unwrap::<FileList>(value) else { return Ok(None) };
    let mut files = FileListBuilder::new(s)?;
    for i in 0..list.len {
        if let Some(f) = list.get(s, i)?.and_then(|v| s.unwrap::<Blob>(v)) {
            files.push(wrap_blob(s, &f)?.value())?;
        }
    }
    Ok(Some(files.finish(s)?))
}

/// What the cross-VM message format keeps of a Blob or File: its bytes,
/// type, and for a File its name and last modification time.
pub(crate) fn blob_parts_of(s: &Scope<'_>, value: Value<'_>) -> Option<(Bytes, String, Option<(String, f64)>)> {
    s.unwrap::<Blob>(value).map(|b| (b.bytes.clone(), b.mime.clone(), b.file.clone()))
}

/// A Blob (or, with `file`, a File) in `s`'s realm.
pub(crate) fn make_blob<'s>(
    s: &Scope<'s>,
    bytes: Bytes,
    mime: String,
    file: Option<(String, f64)>,
) -> JsResult<Value<'s>> {
    Ok(wrap_blob(s, &BlobData { bytes, mime, file })?.value())
}

/// `FileList` of `files` (File wrappers of `s`'s realm).
pub(crate) fn make_file_list<'s>(s: &Scope<'s>, files: pyro_js::Object<'s>) -> JsResult<Value<'s>> {
    let mut list = FileListBuilder::new(s)?;
    let count = files.get_atom("length")?.to_number()? as u32;
    for i in 0..count {
        let file = files.get_index(i)?;
        if s.unwrap::<File>(file).is_some() {
            list.push(file)?;
        }
    }
    list.finish(s)
}

/// The length of FileList `value` (0 for anything else).
pub(crate) fn file_list_len(s: &Scope<'_>, value: Value<'_>) -> usize {
    s.unwrap::<FileList>(value).map_or(0, |l| l.len)
}

/// Whether `value` is a FileList.
pub(crate) fn is_file_list(s: &Scope<'_>, value: Value<'_>) -> bool {
    s.unwrap::<FileList>(value).is_some()
}

/// `input.files` (null for non-file inputs).
pub(crate) fn input_files<'s>(s: &Scope<'s>, n: &NodeData) -> JsResult<Value<'s>> {
    let page = n.page();
    if page.dom.borrow().input_type(n.id).js()? != pyro_dom::forms::InputType::File {
        return Ok(s.null());
    }
    // The same FileList until the selection changes (`PageDom::files_changed`).
    let generation = page.file_generations.borrow().get(&n.id).copied().unwrap_or(0) as f64;
    let input = crate::node::node_value(s, page, Some(n.id))?.as_object();
    if let Some(input) = input {
        let cached = (s.get_hidden(input, INPUT_FILES)?, s.get_hidden(input, INPUT_FILES_GENERATION)?);
        if let (Some(list), Some(g)) = cached {
            if g.to_number()? == generation {
                return Ok(list);
            }
        }
    }
    let entries = page.files.borrow().get(&n.id).cloned().unwrap_or_default();
    let mut files = FileListBuilder::new(s)?;
    for e in entries {
        let f = s.wrap::<File>(
            BlobData { bytes: e.bytes.clone(), mime: e.mime.clone(), file: Some((e.name.clone(), e.last_modified)) },
            None,
        )?;
        files.push(f.value())?;
    }
    let list = files.finish(s)?;
    if let Some(input) = input {
        s.set_hidden(input, INPUT_FILES, list)?;
        s.set_hidden(input, INPUT_FILES_GENERATION, s.number(generation))?;
    }
    Ok(list)
}

/// The FileList an input's `files` returned last, and the selection
/// generation it shows.
const INPUT_FILES: &str = "__lpInputFiles";
const INPUT_FILES_GENERATION: &str = "__lpInputFilesGeneration";

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
    if n.page().dom.borrow().input_type(n.id).js()? != pyro_dom::forms::InputType::File {
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
    let page = n.page();
    page.set_input_files(n.id, entries).js()?;
    // The input's files are this FileList itself, not a copy.
    if let Some(input) = crate::node::node_value(s, page, Some(n.id))?.as_object() {
        let generation = page.file_generations.borrow().get(&n.id).copied().unwrap_or(0) as f64;
        s.set_hidden(input, INPUT_FILES, v)?;
        s.set_hidden(input, INPUT_FILES_GENERATION, s.number(generation))?;
    }
    Ok(())
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<Blob>();
    engine.register::<File>();
    engine.register::<FileList>();
}
