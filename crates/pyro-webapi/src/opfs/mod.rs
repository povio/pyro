//! Origin-private file systems live in the context's storage partition. This
//! bridge exposes native entries to the per-realm Web IDL wrappers, and keeps
//! write locks owned by lease objects until commit, abort or finalization.

mod store;

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use pyro_js::{
    Args, ClassDef, Engine, JsClass, JsError, JsResult, NoParent, Object, Persistent, RootedVec, Scope, Value,
};

use crate::file::{BlobData, File};
use crate::page::PageDom;
pub(crate) use store::Namespace;
use store::{Error, Handle, Kind, Lease, Mode};

const MODULE: &str = "lpOriginPrivateFileSystem";

struct Entry;
impl JsClass for Entry {
    const NAME: &'static str = "OpfsEntry";
    type Data = Handle;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
    }
}

struct Access;
impl JsClass for Access {
    const NAME: &'static str = "OpfsAccess";
    type Data = RefCell<Lease>;
    type Parent = NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.not_exposed();
    }
}

pub(crate) fn register(engine: &Engine) {
    engine.register::<Entry>();
    engine.register::<Access>();
}

fn invalid_handle() -> JsError {
    JsError::Type("Expected a file system handle.".into())
}

fn entry<'s>(s: &Scope<'s>, a: &Args<'s>, index: usize) -> JsResult<&'s Handle> {
    s.unwrap_arg::<Entry>(a, index).ok_or_else(invalid_handle)
}

fn access<'s>(s: &Scope<'s>, a: &Args<'s>) -> JsResult<&'s RefCell<Lease>> {
    s.unwrap_arg::<Access>(a, 0).ok_or_else(|| JsError::Type("Expected an open file access handle.".into()))
}

fn js_error(error: Error) -> JsError {
    match error {
        Error::InvalidName => JsError::Type(error.to_string().into()),
        _ => JsError::dom(error.name(), error.to_string()),
    }
}

fn unsigned(value: Value<'_>) -> JsResult<u64> {
    let invalid = || JsError::Type("File offset is outside the unsigned 64-bit range.".into());
    // Web IDL unsigned long long conversion can produce integers that cannot
    // be represented exactly by a JS number. The wrappers pass decimal text
    // for those, and numbers for ordinary offsets and view lengths.
    if value.is_string() {
        return value.to_string()?.parse().map_err(|_| invalid());
    }
    let n = value.to_number()?;
    if n.fract() == 0.0 && (0.0..18_446_744_073_709_551_616.0).contains(&n) { Ok(n as u64) } else { Err(invalid()) }
}

fn offset(value: Value<'_>) -> JsResult<Option<u64>> {
    if value.is_null() { Ok(None) } else { unsigned(value).map(Some) }
}

fn wrap_entry<'s>(s: &Scope<'s>, handle: Handle) -> JsResult<Value<'s>> {
    Ok(s.wrap::<Entry>(handle, None)?.value())
}

fn natives<'s>(s: &Scope<'s>, page: &PageDom) -> JsResult<Object<'s>> {
    let native = s.object();
    native.set_prototype(s.null());
    native.set_atom("writableSlot", crate::streams::shared_slot(s, page, "FileSystemWritableFileStream")?)?;
    macro_rules! operation {
        ($name:literal, $f:expr) => {
            native.set_atom($name, s.function($name, $f)?.value())?
        };
    }
    operation!("root", |s, _, _| {
        let page = crate::target::page_of(s)?;
        let origin = crate::storage::storage_origin(s, &page)?;
        let storage = page.local_storage.borrow().opfs();
        wrap_entry(s, storage.root(&origin))
    });
    operation!("entry", |s, _, a| Ok(if s.unwrap_arg::<Entry>(&a, 0).is_some() { a.get(0) } else { s.null() }));
    operation!("kind", |s, _, a| Ok(s.string(match entry(s, &a, 0)?.kind() {
        Kind::Directory => "directory",
        Kind::File => "file",
    })));
    operation!("name", |s, _, a| Ok(s.string(&entry(s, &a, 0)?.name())));
    operation!("id", |s, _, a| Ok(s.string(&entry(s, &a, 0)?.unique_id().map_err(js_error)?)));
    operation!("same", |s, _, a| Ok(s.boolean(entry(s, &a, 0)?.is_same_entry(entry(s, &a, 1)?))));
    operation!("find", |s, _, a| {
        let dir = entry(s, &a, 0)?;
        let name = a.get(1).to_string()?;
        let kind = match a.get(2).to_string()?.as_str() {
            "directory" => Kind::Directory,
            "file" => Kind::File,
            _ => return Err(invalid_handle()),
        };
        wrap_entry(s, dir.get(&name, kind, a.get(3).to_bool()).map_err(js_error)?)
    });
    operation!("next", |s, _, a| {
        let after = if a.get(1).is_null() { None } else { Some(a.get(1).to_string()?) };
        match entry(s, &a, 0)?.next(after.as_deref()).map_err(js_error)? {
            None => Ok(s.null()),
            Some((name, child)) => Ok(s.array(&[s.string(&name), wrap_entry(s, child)?])?.value()),
        }
    });
    operation!("resolve", |s, _, a| {
        let Some(names) = entry(s, &a, 0)?.resolve(entry(s, &a, 1)?).map_err(js_error)? else { return Ok(s.null()) };
        let mut path = RootedVec::new();
        for name in names {
            path.push(s, s.string(&name));
        }
        Ok(s.array(&path.iter(s).collect::<Vec<_>>())?.value())
    });
    operation!("remove", |s, _, a| {
        entry(s, &a, 0)?.remove(a.get(1).to_bool()).map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("removeEntry", |s, _, a| {
        entry(s, &a, 0)?.remove_entry(&a.get(1).to_string()?, a.get(2).to_bool()).map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("move", |s, _, a| {
        let to = if a.get(1).is_null() { None } else { Some(entry(s, &a, 1)?) };
        let name = if a.get(2).is_null() { None } else { Some(a.get(2).to_string()?) };
        entry(s, &a, 0)?.move_to(to, name.as_deref()).map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("snapshot", |s, _, a| {
        let file = entry(s, &a, 0)?;
        let (bytes, mtime) = file.snapshot().map_err(js_error)?;
        let name = file.name();
        let content_type = pyro_net::static_files::content_type(Path::new(&name));
        let mime = match content_type.split(';').next().unwrap_or("") {
            "application/octet-stream" => "",
            mime => mime,
        };
        Ok(s.wrap::<File>(BlobData { bytes: bytes.into(), mime: mime.into(), file: Some((name, mtime as f64)) }, None)?
            .value())
    });
    operation!("openWritable", |s, _, a| {
        let exclusive = match a.get(2).to_string()?.as_str() {
            "siloed" => false,
            "exclusive" => true,
            _ => return Err(JsError::Type("Unknown writable file stream mode.".into())),
        };
        let lease = entry(s, &a, 0)?.writable(a.get(1).to_bool(), exclusive).map_err(js_error)?;
        Ok(s.wrap::<Access>(RefCell::new(lease), None)?.value())
    });
    operation!("openSync", |s, _, a| {
        let mode = match a.get(1).to_string()?.as_str() {
            "read-only" => Mode::ReadOnly,
            "readwrite" => Mode::ReadWrite,
            "readwrite-unsafe" => Mode::ReadWriteUnsafe,
            _ => return Err(JsError::Type("Unknown synchronous file access mode.".into())),
        };
        let lease = entry(s, &a, 0)?.sync(mode).map_err(js_error)?;
        Ok(s.wrap::<Access>(RefCell::new(lease), None)?.value())
    });
    operation!("write", |s, _, a| {
        let data = crate::encoding::buffer_source_bytes(s, a.get(1))?
            .ok_or_else(|| JsError::Type("Expected file bytes.".into()))?;
        let at = offset(a.get(2))?;
        let written = access(s, &a)?.borrow_mut().write(&data, at).map_err(js_error)?;
        Ok(s.number(written as f64))
    });
    operation!("read", |s, _, a| {
        let len =
            usize::try_from(unsigned(a.get(1))?).map_err(|_| JsError::Type("Buffer size is too large.".into()))?;
        let at = offset(a.get(2))?;
        let data = access(s, &a)?.borrow_mut().read_bytes(len, at).map_err(js_error)?;
        crate::encoding::uint8_array(s, &data)
    });
    operation!("seek", |s, _, a| {
        let position = unsigned(a.get(1))?;
        access(s, &a)?.borrow_mut().seek(position).map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("truncate", |s, _, a| {
        let size = unsigned(a.get(1))?;
        access(s, &a)?.borrow_mut().truncate(size).map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("size", |s, _, a| {
        let size = access(s, &a)?.borrow_mut().size().map_err(js_error)?;
        Ok(s.number(size as f64))
    });
    operation!("flush", |s, _, a| {
        access(s, &a)?.borrow_mut().flush().map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("close", |s, _, a| {
        access(s, &a)?.borrow_mut().close().map_err(js_error)?;
        Ok(s.undefined())
    });
    operation!("abort", |s, _, a| {
        access(s, &a)?.borrow_mut().abort().map_err(js_error)?;
        Ok(s.undefined())
    });
    Ok(native)
}

pub(crate) fn module<'s>(s: &Scope<'s>, page: &Rc<PageDom>, dedicated: bool) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, MODULE)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let native = Persistent::new(s, natives(s, page)?.value());
    let streams = s.eval(include_str!("streams.js"), Some("pyro://internal/opfs-streams.js"), 1)?;
    let streams = streams.as_object().ok_or_else(invalid_handle)?.call(None, &[native.get(s)])?;
    let streams = Persistent::new(s, streams);
    let factory = s.eval(include_str!("handles.js"), Some("pyro://internal/opfs-handles.js"), 1)?;
    let module = factory
        .as_object()
        .ok_or_else(invalid_handle)?
        .call(None, &[native.get(s), streams.get(s), s.boolean(dedicated)])?;
    s.set_hidden(holder, MODULE, module)?;
    module.as_object().ok_or_else(invalid_handle)
}
