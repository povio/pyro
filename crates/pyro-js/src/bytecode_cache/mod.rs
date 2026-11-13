//! A bytecode cache in files (patched JavaScriptCore, patch 0031).
//!
//! A script or module of some size that an engine evaluates is afterwards
//! compiled once more, whole and off the page's path, and what comes out is
//! written to a file with the source text: the bytecode of the top-level code
//! and of every function, and a module's record (its imports and exports).
//! The next load of the same text, in this process or another, maps the
//! file: the text is not copied, a module is not parsed for its record, and
//! code is decoded from the mapping where it would be parsed and compiled, a
//! function's at its first call.
//!
//! What is decoded lives in the engine's heap like compiled code does; the
//! file replaces the parse and the copy of the text, not that memory.
//!
//! Off unless a directory is given ([`BytecodeCache::open`],
//! [`set_default`]): see [`store`] for why the directory must be private.

pub(crate) mod store;

use std::collections::{HashSet, VecDeque};
use std::ffi::CString;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::ptr;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, RwLock};
use std::time::{Duration, Instant};

use crate::sys;
use sys::spi::{BytecodeCacheSpi, JSCachedSource};

pub(crate) use store::Key;
pub use store::{Rejected, SourceKind, StoreStats};
use store::{Entry, EntryParts, Lookup, SourceText, Store};

/// Sources shorter than this are not cached: opening and checking an entry
/// costs about what parsing this much does.
const MIN_SOURCE_LEN: usize = 4096;
/// Overrides [`MIN_SOURCE_LEN`] (bytes), for measuring where it should be.
const MIN_LEN_ENV: &str = "PYRO_BYTECODE_CACHE_MIN_BYTES";
/// JavaScriptCore's longest string.
const MAX_SOURCE_LEN: usize = i32::MAX as usize;
/// Source text waiting to be compiled for the cache, at most.
const MAX_QUEUED_BYTES: usize = 64 << 20;

/// When entries are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteMode {
    /// Never: the cache is only read.
    Off = 0,
    /// By a thread of its own, with a JavaScript VM of its own, that lives
    /// while there is something to write (the default).
    Background = 1,
    /// By the thread that evaluated the source, before it goes on: for a
    /// process that must stay single-threaded (one that forks) and has the
    /// time (it prepares what others use).
    Inline = 2,
}

struct Job {
    key: Key,
    kind: SourceKind,
    url: String,
    text: String,
}

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    /// Queued or being written.
    pending: HashSet<Key>,
    bytes: usize,
    /// The writer thread is alive.
    running: bool,
}

/// A bytecode cache directory. Shared by the engines of a process.
pub struct BytecodeCache {
    root: PathBuf,
    spi: BytecodeCacheSpi,
    version: Option<u64>,
    /// Opened by the first use: the version depends on JavaScriptCore's
    /// options, which are final once an engine exists. `None`: the directory
    /// could not be used (reported once).
    store: OnceLock<Option<Store>>,
    min_len: AtomicUsize,
    mode: AtomicU8,
    queue: Mutex<Queue>,
    idle: Condvar,
    log: bool,
}

/// What [`BytecodeCache::find`] found.
pub(crate) enum Find {
    /// Not a source the cache handles.
    Skip,
    /// The entry of the source.
    Hit(Key, Arc<Entry>),
    /// No entry: evaluate as without a cache, then [`BytecodeCache::submit`].
    Miss(Key),
}

static DEFAULT: RwLock<Option<Arc<BytecodeCache>>> = RwLock::new(None);

/// The environment variable that names the cache directory of the
/// `pyro` commands (their `--bytecode-cache`).
pub const DIR_ENV: &str = "PYRO_BYTECODE_CACHE";

/// Opens the cache in `dir` and makes it the one engines use
/// ([`BytecodeCache::open`], [`set_default`]).
pub fn use_dir(dir: &Path) -> io::Result<Arc<BytecodeCache>> {
    let cache = BytecodeCache::open(dir)?;
    set_default(Some(cache.clone()));
    Ok(cache)
}

/// Sets the cache that engines created from now on use (`None`: none).
pub fn set_default(cache: Option<Arc<BytecodeCache>>) {
    *DEFAULT.write().unwrap_or_else(std::sync::PoisonError::into_inner) = cache;
}

/// The cache [`set_default`] set.
pub fn default() -> Option<Arc<BytecodeCache>> {
    DEFAULT.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

fn fnv(hash: &mut u64, bytes: &[u8]) {
    for b in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
        *hash = (*hash ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
    }
}

impl BytecodeCache {
    /// Whether this JavaScriptCore can read and write a cache.
    pub fn supported() -> bool {
        sys::spi::get().bytecode_cache().is_some()
    }

    /// The cache in `dir`, which is created if missing and must belong to
    /// the user and be closed to everyone else: its entries are run as code,
    /// unchecked.
    pub fn open(dir: &Path) -> io::Result<Arc<Self>> {
        Self::open_inner(dir, None)
    }

    /// [`BytecodeCache::open`] with `version` in place of the build's: only
    /// entries written under the same value are read.
    pub fn open_with_version(dir: &Path, version: u64) -> io::Result<Arc<Self>> {
        Self::open_inner(dir, Some(version))
    }

    fn open_inner(dir: &Path, version: Option<u64>) -> io::Result<Arc<Self>> {
        let Some(spi) = sys::spi::get().bytecode_cache() else {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the bytecode cache needs the patched JavaScriptCore (scripts/jsc/build.sh)",
            ));
        };
        store::private_dir(dir)?;
        Ok(Arc::new(Self {
            root: dir.to_path_buf(),
            spi,
            version,
            store: OnceLock::new(),
            min_len: AtomicUsize::new(
                std::env::var(MIN_LEN_ENV).ok().and_then(|v| v.trim().parse().ok()).unwrap_or(MIN_SOURCE_LEN),
            ),
            mode: AtomicU8::new(WriteMode::Background as u8),
            queue: Mutex::default(),
            idle: Condvar::new(),
            log: std::env::var_os("PYRO_BYTECODE_CACHE_LOG").is_some(),
        }))
    }

    /// What an entry must have been written under to be read by this build:
    /// the layout of an entry, the JavaScriptCore sources (revision and
    /// patches), JavaScriptCore's own value (its binary and the options
    /// bytecode depends on) and the target.
    pub fn build_version() -> Option<u64> {
        let spi = sys::spi::get().bytecode_cache()?;
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        fnv(&mut hash, &store::FORMAT.to_le_bytes());
        fnv(&mut hash, sys::spi::JSC_INPUTS_HASH.as_bytes());
        // SAFETY: no arguments; reads JavaScriptCore's options.
        fnv(&mut hash, &unsafe { (spi.version)() }.to_le_bytes());
        fnv(&mut hash, std::env::consts::ARCH.as_bytes());
        fnv(&mut hash, std::env::consts::OS.as_bytes());
        fnv(&mut hash, env!("CARGO_PKG_VERSION").as_bytes());
        Some(hash)
    }

    fn store(&self) -> Option<&Store> {
        self.store
            .get_or_init(|| {
                let version = self.version.or_else(Self::build_version)?;
                match Store::open(&self.root, version) {
                    Ok(store) => Some(store),
                    Err(e) => {
                        eprintln!("pyro: bytecode cache {}: {e}; not used", self.root.display());
                        None
                    }
                }
            })
            .as_ref()
    }

    /// The directory given to [`BytecodeCache::open`].
    pub fn dir(&self) -> &Path {
        &self.root
    }

    /// The directory of the entries this build reads and writes. Opens it;
    /// to be called once an engine exists.
    pub fn version_dir(&self) -> Option<&Path> {
        self.store().map(Store::dir)
    }

    /// Sets when entries are written.
    pub fn set_write_mode(&self, mode: WriteMode) {
        self.mode.store(mode as u8, Ordering::Relaxed);
    }

    /// When entries are written.
    pub fn write_mode(&self) -> WriteMode {
        match self.mode.load(Ordering::Relaxed) {
            0 => WriteMode::Off,
            2 => WriteMode::Inline,
            _ => WriteMode::Background,
        }
    }

    /// Sets the length in bytes below which a source is not cached.
    pub fn set_min_source_len(&self, len: usize) {
        self.min_len.store(len, Ordering::Relaxed);
    }

    /// Lookups and writes so far (all zero before the first use).
    pub fn stats(&self) -> StoreStats {
        self.store.get().and_then(Option::as_ref).map(Store::stats).unwrap_or_default()
    }

    /// Waits until everything submitted is written, `timeout` at most.
    /// `false` when it was not.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut queue = self.lock();
        while queue.running || !queue.jobs.is_empty() {
            let Some(left) = deadline.checked_duration_since(Instant::now()) else { return false };
            queue = self.idle.wait_timeout(queue, left).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
        true
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether a source of `len` bytes is one the cache handles.
    pub(crate) fn handles(&self, len: usize) -> bool {
        len >= self.min_len.load(Ordering::Relaxed) && len < MAX_SOURCE_LEN
    }

    /// The entry of the source that `parts` make when concatenated, loaded
    /// as `kind` from `url`.
    pub(crate) fn find(&self, kind: SourceKind, url: &str, parts: &[&str]) -> Find {
        if !self.handles(parts.iter().map(|p| p.len()).sum()) {
            return Find::Skip;
        }
        let Some(store) = self.store() else { return Find::Skip };
        let key = Key::new(kind, &store::url_host(url), parts);
        self.find_key(store, key, kind, url, parts)
    }

    fn find_key(&self, store: &Store, key: Key, kind: SourceKind, url: &str, parts: &[&str]) -> Find {
        match store.lookup(&key, kind, parts) {
            Lookup::Hit(entry) => {
                if self.log {
                    eprintln!("pyro: bytecode cache: hit {key:?} {url}");
                }
                Find::Hit(key, entry)
            }
            Lookup::Miss => {
                if self.log {
                    eprintln!("pyro: bytecode cache: miss {key:?} {url}");
                }
                Find::Miss(key)
            }
            Lookup::OtherText => {
                if self.log {
                    eprintln!("pyro: bytecode cache: other text under {key:?} {url}");
                }
                Find::Skip
            }
            Lookup::Rejected(reason) => {
                eprintln!("pyro: bytecode cache: entry {key:?} removed ({reason:?})");
                Find::Miss(key)
            }
        }
    }

    /// Has the entry for a source [`BytecodeCache::find`] missed written,
    /// as the write mode says. `ctx` is the caller's context, used when the
    /// mode is [`WriteMode::Inline`]; `text` gives the whole source.
    pub(crate) fn submit(
        self: &Arc<Self>,
        ctx: sys::JSContextRef,
        key: Key,
        kind: SourceKind,
        url: &str,
        text: impl FnOnce() -> String,
    ) {
        match self.write_mode() {
            WriteMode::Off => {}
            WriteMode::Inline => {
                self.write(ctx, &Job { key, kind, url: url.to_owned(), text: text() });
            }
            WriteMode::Background => {
                let mut queue = self.lock();
                if queue.pending.contains(&key) {
                    return;
                }
                let text = text();
                if queue.bytes + text.len() > MAX_QUEUED_BYTES {
                    return;
                }
                queue.bytes += text.len();
                queue.pending.insert(key);
                queue.jobs.push_back(Job { key, kind, url: url.to_owned(), text });
                if !queue.running {
                    queue.running = true;
                    let cache = self.clone();
                    let spawned =
                        std::thread::Builder::new().name("pyro bytecode cache".into()).spawn(move || cache.run_writer());
                    if spawned.is_err() {
                        *queue = Queue::default();
                    }
                }
            }
        }
    }

    /// Writes the entry of a module [`BytecodeCache::find`] missed now, on
    /// `ctx`, and returns it.
    pub(crate) fn write_now(
        &self,
        ctx: sys::JSContextRef,
        key: Key,
        kind: SourceKind,
        url: &str,
        text: String,
    ) -> Option<Arc<Entry>> {
        if self.write_mode() == WriteMode::Off {
            return None;
        }
        let job = Job { key, kind, url: url.to_owned(), text };
        self.write(ctx, &job);
        match self.find_key(self.store()?, key, kind, url, &[&job.text]) {
            Find::Hit(_, entry) => Some(entry),
            _ => None,
        }
    }

    /// The writer thread: a VM while there are jobs, gone with the last.
    fn run_writer(self: Arc<Self>) {
        let mut vm: Option<(sys::JSContextGroupRef, sys::JSGlobalContextRef)> = None;
        loop {
            let job = self.lock().jobs.pop_front();
            let Some(job) = job else {
                if let Some((group, ctx)) = vm.take() {
                    // SAFETY: the VM this thread made and alone uses.
                    unsafe {
                        sys::JSGlobalContextRelease(ctx);
                        sys::JSContextGroupRelease(group);
                    }
                }
                let mut queue = self.lock();
                if queue.jobs.is_empty() {
                    queue.running = false;
                    self.idle.notify_all();
                    return;
                }
                continue;
            };
            let (_, ctx) = *vm.get_or_insert_with(|| {
                // SAFETY: plain creation; released above by this thread.
                unsafe {
                    let group = sys::JSContextGroupCreate();
                    (group, sys::JSGlobalContextCreateInGroup(group, ptr::null()))
                }
            });
            self.write(ctx, &job);
            let mut queue = self.lock();
            queue.bytes -= job.text.len();
            queue.pending.remove(&job.key);
        }
    }

    /// Compiles `job` on `ctx` and writes its entry, unless the entry exists
    /// or another writer is at it.
    fn write(&self, ctx: sys::JSContextRef, job: &Job) {
        let Some(store) = self.store() else { return };
        let Some(pending) = store.begin(&job.key) else { return };
        let units;
        let source = if job.text.is_ascii() {
            SourceText::Latin1(job.text.as_bytes())
        } else {
            units = job.text.encode_utf16().collect::<Vec<u16>>();
            SourceText::Utf16(&units)
        };
        let (pointer, length, is_8bit) = raw_text(source);
        let cached = JSCachedSource {
            source: pointer,
            source_length: length,
            source_is_8bit: is_8bit,
            release: None,
            release_info: ptr::null_mut(),
            source_hash: 0,
            bytecode_file_descriptor: -1,
            bytecode_length: 0,
            module_record: ptr::null(),
            module_record_length: 0,
        };
        struct Sink<'a> {
            pending: Option<store::Pending<'a>>,
            kind: SourceKind,
            source: SourceText<'a>,
            written: Option<io::Result<()>>,
        }
        unsafe extern "C" fn sink(
            info: *mut std::ffi::c_void,
            bytecode: *const u8,
            bytecode_length: usize,
            record: *const u8,
            record_length: usize,
            source_hash: std::os::raw::c_uint,
        ) {
            // SAFETY: `info` is the `Sink` of the `write` that is calling
            // JavaScriptCore; the byte ranges are valid during this call.
            unsafe {
                let sink = &mut *info.cast::<Sink<'_>>();
                let Some(pending) = sink.pending.take() else { return };
                let slice = |p: *const u8, len: usize| if p.is_null() { &[][..] } else { std::slice::from_raw_parts(p, len) };
                sink.written = Some(pending.commit(
                    sink.kind,
                    &EntryParts {
                        source: sink.source,
                        source_hash,
                        bytecode: slice(bytecode, bytecode_length),
                        record: slice(record, record_length),
                    },
                ));
            }
        }
        let mut state = Sink { pending: Some(pending), kind: job.kind, source, written: None };
        let url = if job.url.is_empty() { None } else { CString::new(job.url.as_str()).ok() };
        // SAFETY: `ctx` is a live context of the calling thread's VM; the
        // text and `state` outlive the call, which copies the text.
        let generated = unsafe {
            let url = url.as_ref().map_or(ptr::null(), |u| sys::JSStringCreateWithUTF8CString(u.as_ptr()));
            let generated = (self.spi.generate)(
                ctx,
                &cached,
                url,
                job.kind == SourceKind::Module,
                sink,
                ptr::from_mut(&mut state).cast(),
            );
            if !url.is_null() {
                sys::JSStringRelease(url);
            }
            generated
        };
        if self.log {
            eprintln!(
                "pyro: bytecode cache: write {:?} {} ({} bytes): {}",
                job.key,
                job.url,
                job.text.len(),
                match (&state.written, generated) {
                    (Some(Ok(())), _) => "written".to_owned(),
                    (Some(Err(e)), _) => format!("failed: {e}"),
                    (None, false) => "not compiled".to_owned(),
                    (None, true) => "nothing to write".to_owned(),
                }
            );
        }
    }

    pub(crate) fn spi(&self) -> &BytecodeCacheSpi {
        &self.spi
    }
}

fn raw_text(text: SourceText<'_>) -> (*const std::ffi::c_void, usize, bool) {
    match text {
        SourceText::Latin1(b) => (b.as_ptr().cast(), b.len(), true),
        SourceText::Utf16(u) => (u.as_ptr().cast(), u.len(), false),
    }
}

unsafe extern "C" fn release_entry(info: *mut std::ffi::c_void) {
    // SAFETY: the reference `CachedSource::new` leaked for this call.
    drop(unsafe { Arc::from_raw(info.cast::<Entry>().cast_const()) });
}

/// An entry as JavaScriptCore takes it. To be given to exactly one call of
/// JavaScriptCore, which releases the text: one not given leaks the entry's
/// mapping.
pub(crate) struct CachedSource {
    pub(crate) raw: JSCachedSource,
    /// Open during the call, which maps the bytecode from it.
    _file: Option<std::fs::File>,
}

impl CachedSource {
    /// `entry`'s text, kept mapped by JavaScriptCore for as long as it
    /// refers to it, with the entry's bytecode and record when `code`.
    pub(crate) fn new(entry: &Arc<Entry>, code: bool) -> Self {
        let (source, source_length, source_is_8bit) = raw_text(entry.source());
        let file = if code { entry.bytecode_file() } else { None };
        let record = if code { entry.record() } else { &[] };
        Self {
            raw: JSCachedSource {
                source,
                source_length,
                source_is_8bit,
                release: Some(release_entry),
                release_info: Arc::into_raw(entry.clone()).cast_mut().cast(),
                source_hash: entry.source_hash(),
                bytecode_file_descriptor: file.as_ref().map_or(-1, AsRawFd::as_raw_fd),
                bytecode_length: if file.is_some() { entry.bytecode_len() } else { 0 },
                module_record: if record.is_empty() { ptr::null() } else { record.as_ptr() },
                module_record_length: record.len(),
            },
            _file: file,
        }
    }
}
