//! Bounded, task-driven reads of the native immutable Blob/File byte store.
//! Pending operations pin their reader; Blob, result and error references are
//! traced wrapper properties. Queued tasks hold only weak operation references.
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    rc::{Rc, Weak},
};

use base64::Engine as _;
use bytes::Bytes;
use pyro_dom::events::TargetKey;
use pyro_js::{
    ClassDef, IdentityKey, JsClass, JsError, JsResult, Object, Persistent, RealmId, Scope, Value, WeakRealm,
};
use rustc_hash::FxHashMap;

use crate::{
    event::{EventCore, EventInterface},
    page::PageDom,
    target::{self, EventTarget, EventTargetData},
};

const BLOB: &str = "__lpReaderBlob";
const RESULT: &str = "__lpReaderResult";
const ERROR: &str = "__lpReaderError";

/// Per-document input bounds. Retained JS results are outside this pending budget.
#[derive(Clone, Copy, Debug)]
pub struct FileReaderLimits {
    pub max_read_bytes: usize,
    pub max_pending_bytes: usize,
    pub max_pending_reads: usize,
}
impl Default for FileReaderLimits {
    fn default() -> Self {
        Self { max_read_bytes: 64 * 1024 * 1024, max_pending_bytes: 64 * 1024 * 1024, max_pending_reads: 128 }
    }
}
#[derive(Default)]
pub(crate) struct ReaderState {
    limits: Cell<FileReaderLimits>,
    jobs: RefCell<FxHashMap<IdentityKey, Rc<Pending>>>,
    bytes: Cell<usize>,
    tasks: RefCell<VecDeque<(IdentityKey, u64, u8)>>,
    posted: Cell<bool>,
}
impl PageDom {
    /// Limits may change only while there are no pending reads.
    pub fn set_file_reader_limits(&self, limits: FileReaderLimits) -> bool {
        if !self.file_readers.jobs.borrow().is_empty() {
            return false;
        }
        self.file_readers.limits.set(limits);
        true
    }
    /// Pending operations and their retained logical input bytes.
    pub fn file_reader_usage(&self) -> (usize, usize) {
        (self.file_readers.jobs.borrow().len(), self.file_readers.bytes.get())
    }
}

pub struct ReaderData {
    target: EventTargetData,
    reader: Rc<Reader>,
}
struct Reader {
    page: Weak<PageDom>,
    realm: WeakRealm,
    realm_id: RealmId,
    identity: IdentityKey,
    key: TargetKey,
    ready: Cell<u8>,
    generation: Cell<u64>,
}
enum Mode {
    Buffer,
    Text(Option<String>),
    DataUrl,
    Binary,
}
impl Mode {
    fn convert<'s>(&self, s: &Scope<'s>, bytes: &[u8], mime: &str) -> JsResult<Value<'s>> {
        Ok(match self {
            Mode::Buffer => crate::encoding::array_buffer(s, bytes)?,
            Mode::Binary => s.string(&bytes.iter().map(|b| char::from(*b)).collect::<String>()),
            Mode::DataUrl => {
                let mime = if mime.is_empty() { "application/octet-stream" } else { mime };
                s.string(&format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
            }
            Mode::Text(label) => {
                let encoding = label
                    .as_ref()
                    .and_then(|v| encoding_rs::Encoding::for_label(v.as_bytes()))
                    .or_else(|| {
                        mime.parse::<data_url::mime::Mime>()
                            .ok()
                            .and_then(|m| encoding_rs::Encoding::for_label(m.get_parameter("charset")?.as_bytes()))
                    })
                    .unwrap_or(encoding_rs::UTF_8);
                s.string(&encoding.decode(bytes).0)
            }
        })
    }
}

struct Pending {
    reader: Rc<Reader>,
    _wrapper: Persistent,
    generation: u64,
    bytes: RefCell<Option<Bytes>>,
    mime: String,
    mode: Mode,
    total: usize,
    reserved: usize,
}

pub struct FileReader;
impl JsClass for FileReader {
    const NAME: &'static str = "FileReader";
    type Data = ReaderData;
    type Parent = EventTarget;
    fn define(def: &mut ClassDef<Self>) {
        def.upcast(|d: &ReaderData| &d.target);
        def.constructor(0, |s, nt, _| {
            let page = target::page_of(s)?;
            if page.is_disposed() {
                return Err(JsError::ContextDestroyed);
            }
            let target = EventTargetData::new(&page);
            let TargetKey::Other(id) = target.key else { unreachable!() };
            let identity = page.other_identity(id);
            let reader = Rc::new(Reader {
                page: Rc::downgrade(&page),
                realm: s.realm().downgrade(),
                realm_id: s.realm_id(),
                identity,
                key: target.key,
                ready: Cell::new(0),
                generation: Cell::new(0),
            });
            s.wrap_new::<Self>(ReaderData { target, reader }, nt, Some(identity))
        });
        for (name, n) in [("EMPTY", 0.), ("LOADING", 1.), ("DONE", 2.)] {
            def.constant(name, n);
        }
        def.getter("readyState", |s, d| Ok(s.number(d.reader.ready.get().into())));
        def.getter("result", |s, d| d.reader.value(s, RESULT));
        def.getter("error", |s, d| d.reader.value(s, ERROR));
        def.method("readAsArrayBuffer", 1, |s, d, a| read(s, d, a, Mode::Buffer));
        def.method("readAsDataURL", 1, |s, d, a| read(s, d, a, Mode::DataUrl));
        def.method("readAsBinaryString", 1, |s, d, a| read(s, d, a, Mode::Binary));
        def.method("readAsText", 1, |s, d, a| {
            a.require(1, "readAsText")?;
            if s.unwrap::<crate::file::Blob>(a.get(0)).is_none() {
                return Err(JsError::Type("FileReader requires a Blob or File".into()));
            }
            // String conversion may run author JS; no state is borrowed yet.
            let label = a.opt(1).filter(|v| !v.is_undefined()).map(crate::util::dom_string).transpose()?;
            read(s, d, a, Mode::Text(label))
        });
        def.method("abort", 0, |s, d, _| {
            d.reader.abort(s)?;
            Ok(s.undefined())
        });
        macro_rules! handlers {
            ($($ty:literal),*) => { $(def.accessor(concat!("on", $ty),
                |s, d| target::event_handler(s, &d.target, $ty),
                |s, d, v| target::set_event_handler(s, &d.target, $ty, v));)* };
        }
        handlers!("loadstart", "progress", "load", "error", "abort", "loadend");
    }
}

/// Synchronous reads are available to dedicated and shared workers.
pub(crate) struct FileReaderSync;
impl JsClass for FileReaderSync {
    const NAME: &'static str = "FileReaderSync";
    type Data = ();
    type Parent = pyro_js::NoParent;

    fn define(def: &mut ClassDef<Self>) {
        def.constructor(0, |s, nt, _| s.wrap_new::<Self>((), nt, None));
        def.method("readAsArrayBuffer", 1, |s, _, a| read_sync(s, a, Mode::Buffer));
        def.method("readAsBinaryString", 1, |s, _, a| read_sync(s, a, Mode::Binary));
        def.method("readAsDataURL", 1, |s, _, a| read_sync(s, a, Mode::DataUrl));
        def.method("readAsText", 1, |s, _, a| read_sync(s, a, Mode::Text(None)));
    }
}

fn read_sync<'s>(s: &Scope<'s>, a: pyro_js::Args<'s>, mut mode: Mode) -> JsResult<Value<'s>> {
    a.require(1, "FileReaderSync read")?;
    let (bytes, mime) = {
        let blob = s
            .unwrap::<crate::file::Blob>(a.get(0))
            .ok_or_else(|| JsError::Type("FileReaderSync requires a Blob or File".into()))?;
        (blob.bytes.clone(), blob.mime.clone())
    };
    if let Mode::Text(label) = &mut mode {
        *label = a.opt(1).filter(|v| !v.is_undefined()).map(crate::util::dom_string).transpose()?;
    }
    if bytes.len() > target::page_of(s)?.file_readers.limits.get().max_read_bytes {
        return Err(JsError::dom("NotReadableError", "FileReader input budget exceeded"));
    }
    mode.convert(s, &bytes, &mime)
}

fn read<'s>(s: &Scope<'s>, d: &ReaderData, a: pyro_js::Args<'s>, mode: Mode) -> JsResult<Value<'s>> {
    a.require(1, "FileReader read")?;
    if s.unwrap::<crate::file::Blob>(a.get(0)).is_none() {
        return Err(JsError::Type("FileReader requires a Blob or File".into()));
    }
    d.reader.read(s, a.get(0), mode)?;
    Ok(s.undefined())
}
impl Reader {
    fn wrapper<'s>(&self, s: &Scope<'s>) -> JsResult<Object<'s>> {
        let realm = self.realm.upgrade().ok_or(JsError::ContextDestroyed)?;
        let owner = realm.scope()?;
        let wrapper = owner.lookup_identity(self.identity).ok_or(JsError::ContextDestroyed)?;
        // SAFETY: receiver or Pending roots the wrapper in this same VM.
        Ok(unsafe { Object::from_raw(s.ctx(), wrapper.as_raw()) })
    }
    fn value<'s>(&self, s: &Scope<'s>, key: &'static str) -> JsResult<Value<'s>> {
        Ok(s.get_hidden(self.wrapper(s)?, key)?.unwrap_or(s.null()))
    }
    fn read(self: &Rc<Self>, s: &Scope<'_>, blob: Value<'_>, mode: Mode) -> JsResult<()> {
        if self.ready.get() == 1 {
            return Err(JsError::dom("InvalidStateError", "FileReader is already loading"));
        }
        let page = self.page.upgrade().filter(|p| !p.is_disposed()).ok_or(JsError::ContextDestroyed)?;
        if page.hooks().is_none() {
            return Err(JsError::dom("NotSupportedError", "FileReader requires queued PageHooks tasks"));
        }
        let limits = page.file_readers.limits.get();
        if page.file_readers.jobs.borrow().len() >= limits.max_pending_reads {
            return Err(JsError::dom("QuotaExceededError", "Too many pending FileReader operations"));
        }
        let source = s.unwrap::<crate::file::Blob>(blob).expect("validated Blob");
        let total = source.bytes.len();
        let allowed = total <= limits.max_read_bytes
            && total <= limits.max_pending_bytes.saturating_sub(page.file_readers.bytes.get());
        let wrapper = self.wrapper(s)?;
        s.set_hidden(wrapper, RESULT, s.null())?;
        s.set_hidden(wrapper, ERROR, s.null())?;
        s.set_hidden(wrapper, BLOB, if allowed { blob } else { s.null() })?;
        let generation = self.generation.get().wrapping_add(1);
        self.generation.set(generation);
        self.ready.set(1);
        let pending = Rc::new(Pending {
            reader: self.clone(),
            _wrapper: Persistent::new(s, wrapper.value()),
            generation,
            bytes: RefCell::new(allowed.then(|| source.bytes.clone())),
            mime: source.mime.clone(),
            mode,
            total,
            reserved: if allowed { total } else { 0 },
        });
        page.file_readers.bytes.set(page.file_readers.bytes.get() + pending.reserved);
        page.file_readers.jobs.borrow_mut().insert(self.identity, pending.clone());
        pending.schedule(&page, 0);
        Ok(())
    }
    fn abort(&self, s: &Scope<'_>) -> JsResult<()> {
        s.set_hidden(self.wrapper(s)?, RESULT, s.null())?;
        if self.ready.get() != 1 {
            return Ok(());
        }
        self.ready.set(2);
        self.generation.set(self.generation.get().wrapping_add(1));
        let generation = self.generation.get();
        let page = self.page.upgrade().ok_or(JsError::ContextDestroyed)?;
        let pending = take_pending(&page, self.identity);
        s.set_hidden(self.wrapper(s)?, BLOB, s.null())?;
        if let Some(p) = &pending {
            p.bytes.take();
        }
        self.fire(&page, "abort", 0, 0, false);
        if self.generation.get() == generation && self.ready.get() != 1 {
            self.fire(&page, "loadend", 0, 0, false);
        }
        Ok(())
    }
    fn fire(&self, page: &Rc<PageDom>, ty: &str, loaded: usize, total: usize, computable: bool) {
        if page.is_disposed() || self.realm.upgrade().is_none_or(|r| r.is_disposed()) {
            return;
        }
        let ev = EventCore::trusted(page, EventInterface::ProgressEvent, ty, false, false);
        {
            let mut f = ev.fields.borrow_mut();
            f.loaded = loaded as f64;
            f.total = total as f64;
            f.length_computable = computable;
        }
        target::fire(page, self.key, &ev);
    }
}
fn take_pending(page: &PageDom, identity: IdentityKey) -> Option<Rc<Pending>> {
    let pending = page.file_readers.jobs.borrow_mut().remove(&identity);
    page.file_readers.tasks.borrow_mut().retain(|(id, _, _)| *id != identity);
    if let Some(p) = &pending {
        page.file_readers.bytes.set(page.file_readers.bytes.get() - p.reserved);
    }
    pending
}
impl Pending {
    fn current(&self, page: &PageDom) -> bool {
        !page.is_disposed() && self.reader.ready.get() == 1 && self.reader.generation.get() == self.generation
    }
    fn schedule(self: &Rc<Self>, page: &Rc<PageDom>, phase: u8) {
        page.file_readers.tasks.borrow_mut().push_back((self.reader.identity, self.generation, phase));
        post_next(page);
    }
    fn run(self: &Rc<Self>, phase: u8) {
        let Some(page) = self.reader.page.upgrade() else { return };
        if !self.current(&page) {
            return;
        }
        let Some(realm) = self.reader.realm.upgrade() else {
            cancel(&page, self);
            return;
        };
        let Ok(s) = realm.scope() else {
            cancel(&page, self);
            return;
        };
        match phase {
            0 => {
                self.reader.fire(&page, "loadstart", 0, self.total, true);
                if self.current(&page) {
                    self.schedule(&page, 1);
                }
            }
            1 if self.bytes.borrow().is_some() => {
                self.reader.fire(&page, "progress", self.total, self.total, true);
                if self.current(&page) {
                    self.schedule(&page, 2);
                }
            }
            _ => self.finish(&s, &page),
        }
    }
    fn finish(&self, s: &Scope<'_>, page: &Rc<PageDom>) {
        let result = self
            .bytes
            .take()
            .ok_or_else(|| JsError::dom("NotReadableError", "FileReader input budget exceeded"))
            .and_then(|bytes| self.mode.convert(s, &bytes, &self.mime));
        let Ok(wrapper) = self.reader.wrapper(s) else {
            cancel(page, self);
            return;
        };
        let (value, error, ty) = match result {
            Ok(v) => (v, s.null(), "load"),
            Err(_) => {
                (s.null(), JsError::dom("NotReadableError", "FileReader could not read the Blob").to_value(s), "error")
            }
        };
        if s.set_hidden(wrapper, RESULT, value)
            .and_then(|_| s.set_hidden(wrapper, ERROR, error))
            .and_then(|_| s.set_hidden(wrapper, BLOB, s.null()))
            .is_err()
        {
            cancel(page, self);
            return;
        }
        self.reader.ready.set(2);
        let _keep = take_pending(page, self.reader.identity);
        let loaded = if ty == "load" { self.total } else { 0 };
        self.reader.fire(page, ty, loaded, self.total, true);
        if self.reader.generation.get() == self.generation && self.reader.ready.get() != 1 {
            self.reader.fire(page, "loadend", loaded, self.total, true);
        }
    }
}
fn cancel(page: &PageDom, pending: &Pending) {
    pending.reader.ready.set(2);
    pending.reader.generation.set(pending.reader.generation.get().wrapping_add(1));
    pending.bytes.take();
    if let Some(realm) = pending.reader.realm.upgrade() {
        if let Ok(s) = realm.scope() {
            if let Ok(wrapper) = pending.reader.wrapper(&s) {
                let _ = s.set_hidden(wrapper, BLOB, s.null());
                let _ = s.set_hidden(wrapper, RESULT, s.null());
            }
        }
    }
    drop(take_pending(page, pending.reader.identity));
}
pub(crate) fn cancel_realm(page: &PageDom, realm: RealmId) {
    let jobs: Vec<_> =
        page.file_readers.jobs.borrow().values().filter(|p| p.reader.realm_id == realm).cloned().collect();
    for job in jobs {
        cancel(page, &job);
    }
}
pub(crate) fn dispose(page: &PageDom) {
    let jobs: Vec<_> = page.file_readers.jobs.borrow().values().cloned().collect();
    for job in jobs {
        cancel(page, &job);
    }
}

// One outstanding owner task per document bounds aborted-read tombstones too.
fn post_next(page: &Rc<PageDom>) {
    if page.is_disposed() || page.file_readers.tasks.borrow().is_empty() || page.file_readers.posted.replace(true) {
        return;
    }
    let weak = Rc::downgrade(page);
    page.post_task(Box::new(move || {
        let Some(page) = weak.upgrade() else { return };
        page.file_readers.posted.set(false);
        let next = page.file_readers.tasks.borrow_mut().pop_front();
        if let Some((id, generation, phase)) = next {
            let pending = page.file_readers.jobs.borrow().get(&id).cloned();
            if let Some(pending) = pending.filter(|p| p.generation == generation) {
                pending.run(phase);
            }
        }
        post_next(&page);
    }));
}
