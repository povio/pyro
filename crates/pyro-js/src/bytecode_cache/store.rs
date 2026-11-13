//! The files of the bytecode cache: one entry per source, mapped read-only.
//!
//! An entry is trusted input to JavaScriptCore: its bytecode is decoded
//! without validation, so a wrong byte is memory-unsafe. What keeps a wrong
//! byte out:
//!
//! - the directory and every entry belong to the user and are closed to
//!   everyone else (0700 and 0600, checked when opened, symlinks refused);
//! - an entry appears under its name complete or not at all (written under a
//!   temporary name, then renamed), and is never modified afterwards;
//! - an open of an entry checks its length, format, version, key and a
//!   checksum of the whole file, and a mismatch removes the file;
//! - an entry is given for a source only if its text is that source's, byte
//!   for byte: JavaScriptCore does not compare the text again, and the name
//!   of an entry (a hash of kind, host and text, made to be fast) is not
//!   proof, since a page chooses its scripts' text.
//!
//! An entry's file starts with the bytecode, so that JavaScriptCore maps the
//! head of the file; the description of the file is at its end, where a
//! truncated file does not have it.

use std::collections::HashMap;
use std::ffi::CString;
use std::fs;
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Once;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, SystemTime};

use xxhash_rust::xxh3::Xxh3;

const MAGIC: [u8; 8] = *b"LPBCACHE";
/// The layout below. Part of the version, so a change orphans old entries.
pub(crate) const FORMAT: u32 = 3;
const TRAILER_LEN: usize = 128;
/// Sections start on this boundary (UTF-16 text is read in place).
const ALIGN: usize = 16;
/// No entry is larger (a bound for the lengths read from a trailer).
const MAX_ENTRY_LEN: u64 = 1 << 30;
/// Version directories of other builds are removed once unused this long.
const STALE_VERSION_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
/// A temporary file this old belongs to a writer that died.
const STALE_TEMP_AGE: Duration = Duration::from_secs(600);

const FLAG_SOURCE_8BIT: u32 = 1 << 8;

// Trailer offsets.
const OFF_FORMAT: usize = 8;
const OFF_FLAGS: usize = 12;
const OFF_KEY: usize = 16;
const OFF_VERSION: usize = 48;
const OFF_CHECKSUM: usize = 56;
const OFF_FILE_LEN: usize = 64;
const OFF_SOURCE_HASH: usize = 72;
const OFF_BYTECODE_LEN: usize = 80;
const OFF_SOURCE: usize = 88;
const OFF_RECORD: usize = 104;

/// What a source is compiled as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SourceKind {
    /// A classic script.
    Classic = 0,
    /// A module script.
    Module = 1,
}

/// The host of `url` as far as telling two apart goes: the authority without
/// user and port, lowercased; empty for a string without one. JavaScriptCore
/// keeps a script's host in the key of its code, so it is part of an entry's
/// name. Telling apart more than JavaScriptCore does costs an entry, and
/// less a refused one: neither is unsafe.
pub fn url_host(url: &str) -> String {
    let Some((_, rest)) = url.split_once("://") else { return String::new() };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = match host.strip_prefix('[') {
        // An IPv6 literal keeps its colons.
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    host.to_ascii_lowercase()
}

/// The name of a cached source: a hash (XXH3, 128 bits) of kind, host and
/// text. Not its identity: see [`Entry::has_text`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Key([u8; 16]);

impl Key {
    /// The key of the text `parts` make when concatenated, loaded as `kind`
    /// from `host` ([`url_host`]).
    pub fn new(kind: SourceKind, host: &str, parts: &[&str]) -> Self {
        let mut h = Xxh3::new();
        h.update(&[kind as u8]);
        h.update(&(host.len() as u64).to_le_bytes());
        h.update(host.as_bytes());
        for part in parts {
            h.update(part.as_bytes());
        }
        Self(h.digest128().to_le_bytes())
    }

    fn file_name(&self) -> String {
        let mut name = String::with_capacity(32);
        for b in self.0 {
            name.push(char::from(b"0123456789abcdef"[usize::from(b >> 4)]));
            name.push(char::from(b"0123456789abcdef"[usize::from(b & 15)]));
        }
        name
    }
}

impl std::fmt::Debug for Key {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.file_name())
    }
}

/// A read-only private mapping of part of a file.
struct Map {
    ptr: *mut libc::c_void,
    len: usize,
}

// SAFETY: the mapping is read-only and never changes; it is unmapped by the
// one owner.
unsafe impl Send for Map {}
unsafe impl Sync for Map {}

impl Map {
    /// Maps `len` bytes of `file` from `offset`, a multiple of the page size.
    fn new(file: &fs::File, offset: u64, len: usize) -> io::Result<Self> {
        // SAFETY: a new read-only mapping of an open file.
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ,
                libc::MAP_PRIVATE,
                file.as_raw_fd(),
                offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { ptr, len })
    }

    fn bytes(&self) -> &[u8] {
        // SAFETY: `len` mapped readable bytes that live as long as `self`.
        unsafe { std::slice::from_raw_parts(self.ptr.cast(), self.len) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: the mapping made in `new`, unmapped once.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

fn page_size() -> u64 {
    // SAFETY: a constant of the system.
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    u64::try_from(size).ok().filter(|s| s.is_power_of_two()).unwrap_or(4096)
}

/// A source text as JavaScriptCore stores it.
#[derive(Clone, Copy, Debug)]
pub enum SourceText<'a> {
    /// One byte per character (the text is ASCII or Latin-1).
    Latin1(&'a [u8]),
    /// UTF-16 code units.
    Utf16(&'a [u16]),
}

impl SourceText<'_> {
    fn bytes(&self) -> &[u8] {
        match self {
            Self::Latin1(b) => b,
            // SAFETY: the bytes of a `u16` slice; any byte pattern is a `u8`.
            Self::Utf16(u) => unsafe { std::slice::from_raw_parts(u.as_ptr().cast(), std::mem::size_of_val(*u)) },
        }
    }
}

/// What an entry is made of.
pub struct EntryParts<'a> {
    /// The text JavaScriptCore compiled: a later load reads it from the
    /// entry's mapping instead of keeping a copy.
    pub source: SourceText<'a>,
    /// JavaScriptCore's hash of that text (0 if unknown).
    pub source_hash: u32,
    /// The encoded unlinked bytecode.
    pub bytecode: &'a [u8],
    /// A module's imports and exports (empty for a classic script).
    pub record: &'a [u8],
}

/// What tells a file from the one that replaces it under the same name.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Identity {
    dev: u64,
    ino: u64,
    len: u64,
    mtime: (i64, i64),
}

impl Identity {
    fn of(meta: &fs::Metadata) -> Self {
        Self { dev: meta.dev(), ino: meta.ino(), len: meta.len(), mtime: (meta.mtime(), meta.mtime_nsec()) }
    }
}

/// A validated entry. Its source text stays mapped while it lives.
pub struct Entry {
    path: PathBuf,
    identity: Identity,
    /// The file as opened for validation, for the first [`Entry::bytecode_file`].
    file: Mutex<Option<fs::File>>,
    /// Covers the source text (from the start of the page it starts in).
    text: Map,
    text_offset: usize,
    text_bytes: usize,
    text_is_8bit: bool,
    source_hash: u32,
    bytecode_len: usize,
    record: Box<[u8]>,
}

impl Entry {
    /// The source text, in the mapping.
    pub fn source(&self) -> SourceText<'_> {
        let bytes = &self.text.bytes()[self.text_offset..self.text_offset + self.text_bytes];
        if self.text_is_8bit {
            SourceText::Latin1(bytes)
        } else {
            // SAFETY: the section starts on a 16-byte boundary of the file,
            // whose mapping starts on a page, and has an even length
            // (checked at open).
            SourceText::Utf16(unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast(), bytes.len() / 2) })
        }
    }

    /// Whether the entry's text is the one `parts` make when concatenated.
    /// What makes an entry the entry of a source: a key can be shared by two
    /// texts, and a page that wanted another's entry would look for them.
    pub fn has_text(&self, parts: &[&str]) -> bool {
        match self.source() {
            SourceText::Latin1(mut text) => {
                // Written for ASCII text only, where a byte is a character.
                for part in parts {
                    let Some((head, rest)) = text.split_at_checked(part.len()) else { return false };
                    if head != part.as_bytes() {
                        return false;
                    }
                    text = rest;
                }
                text.is_empty()
            }
            SourceText::Utf16(text) => text.iter().copied().eq(parts.iter().flat_map(|p| p.encode_utf16())),
        }
    }

    /// JavaScriptCore's hash of the source text, 0 if unknown.
    pub fn source_hash(&self) -> u32 {
        self.source_hash
    }

    /// The length of the encoded bytecode, which is the head of the file.
    pub fn bytecode_len(&self) -> usize {
        self.bytecode_len
    }

    /// The validated file, to map the bytecode from. `None` once another
    /// file has the entry's name (the entry was removed or written again).
    pub fn bytecode_file(&self) -> Option<fs::File> {
        if let Some(file) = self.file.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take() {
            return Some(file);
        }
        let file = open_private(&self.path).ok()?;
        (Identity::of(&file.metadata().ok()?) == self.identity).then_some(file)
    }

    /// The module record (empty for a classic script).
    pub fn record(&self) -> &[u8] {
        &self.record
    }
}

/// Why an existing file was not used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejected {
    /// Not a regular file of this user that only they can access.
    NotPrivate,
    /// Shorter than a trailer, or than its trailer says.
    Truncated,
    /// Not an entry of this format (a truncated entry among them: its end is
    /// not a trailer).
    Format,
    /// Written by another build or under other options.
    Version,
    /// An entry of another key or kind.
    Key,
    /// A section lies outside the file or is misaligned.
    Layout,
    /// The contents do not match the checksum.
    Checksum,
    /// The file could not be read.
    Io,
}

/// The result of [`Store::lookup`].
pub enum Lookup {
    /// A valid entry.
    Hit(Arc<Entry>),
    /// No file for the key.
    Miss,
    /// The key's entry is of another text: this source has none and gets none.
    OtherText,
    /// A file that was not used, and why (it has been removed).
    Rejected(Rejected),
}

/// Counters of a [`Store`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    /// Lookups answered with an entry.
    pub hits: u64,
    /// Those of them answered with an entry the process already had mapped.
    pub mapped_hits: u64,
    /// Lookups that found no file.
    pub misses: u64,
    /// Lookups that found a file and refused it.
    pub rejected: u64,
    /// Entries written.
    pub written: u64,
    /// Entries that could not be written.
    pub write_errors: u64,
}

#[derive(Default)]
struct Counters {
    hits: AtomicU64,
    mapped_hits: AtomicU64,
    misses: AtomicU64,
    rejected: AtomicU64,
    written: AtomicU64,
    write_errors: AtomicU64,
}

/// The entries of one cache directory for one version.
pub struct Store {
    dir: PathBuf,
    version: u64,
    /// The entries something still holds. They were validated when mapped,
    /// and a process forked from this one inherits them: another load of the
    /// same source opens and reads nothing, and gets the same text.
    open: Mutex<HashMap<Key, Weak<Entry>>>,
    counters: Counters,
    /// The directory was tidied ([`remove_stale`]): by the first write, so
    /// that a process that only reads lists no directory.
    tidied: Once,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("four bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("eight bytes"))
}

fn euid() -> u32 {
    // SAFETY: no arguments, cannot fail.
    unsafe { libc::geteuid() }
}

/// Creates `dir` if missing (0700) and checks that it is a directory of this
/// user closed to everyone else, and not a symbolic link.
pub(crate) fn private_dir(dir: &Path) -> io::Result<()> {
    match fs::DirBuilder::new().recursive(true).mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = fs::symlink_metadata(dir)?;
    if !meta.file_type().is_dir() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("{} is not a directory", dir.display())));
    }
    if meta.uid() != euid() || meta.mode() & 0o077 != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "{} must belong to the current user and be closed to others (mode 0700): its entries are run as code",
                dir.display()
            ),
        ));
    }
    Ok(())
}

/// Opens an entry for reading without following a symbolic link.
fn open_private(path: &Path) -> io::Result<fs::File> {
    fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)
}

impl Store {
    /// Opens (creating it if needed) the cache in `root` for entries of
    /// `version`. Entries of other versions live in directories of their own
    /// and are never read; the first write removes those unused for two weeks.
    pub fn open(root: &Path, version: u64) -> io::Result<Self> {
        private_dir(root)?;
        let name = format!("v-{version:016x}");
        let dir = root.join(&name);
        private_dir(&dir)?;
        // Mark this version as in use, then drop the ones nobody used lately.
        if let Ok(path) = CString::new(dir.as_os_str().as_bytes()) {
            // SAFETY: a NUL-terminated path; null times mean "now".
            unsafe { libc::utimensat(libc::AT_FDCWD, path.as_ptr(), std::ptr::null(), 0) };
        }
        Ok(Self { dir, version, open: Mutex::default(), counters: Counters::default(), tidied: Once::new() })
    }

    /// The directory that holds this version's entries.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The path of the entry for `key`.
    pub fn entry_path(&self, key: &Key) -> PathBuf {
        self.dir.join(key.file_name())
    }

    fn temp_path(&self, key: &Key) -> PathBuf {
        self.dir.join(format!(".{}.tmp", key.file_name()))
    }

    /// The entry for `key`, validated, if it is the entry of the text `parts`
    /// make. A file that fails validation is removed, so that the source is
    /// cached anew.
    pub fn lookup(&self, key: &Key, kind: SourceKind, parts: &[&str]) -> Lookup {
        let mapped = self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(key).and_then(Weak::upgrade);
        if let Some(entry) = mapped {
            if !entry.has_text(parts) {
                return Lookup::OtherText;
            }
            self.counters.hits.fetch_add(1, Ordering::Relaxed);
            self.counters.mapped_hits.fetch_add(1, Ordering::Relaxed);
            return Lookup::Hit(entry);
        }
        let path = self.entry_path(key);
        match self.open_entry(&path, key, kind) {
            Ok(Some(entry)) => {
                if !entry.has_text(parts) {
                    return Lookup::OtherText;
                }
                let entry = Arc::new(entry);
                let mut open = self.open.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if open.len() >= 64 && open.len().is_power_of_two() {
                    open.retain(|_, e| e.strong_count() > 0);
                }
                open.insert(*key, Arc::downgrade(&entry));
                self.counters.hits.fetch_add(1, Ordering::Relaxed);
                Lookup::Hit(entry)
            }
            Ok(None) => {
                self.counters.misses.fetch_add(1, Ordering::Relaxed);
                Lookup::Miss
            }
            Err(reason) => {
                let _ = fs::remove_file(&path);
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                Lookup::Rejected(reason)
            }
        }
    }

    fn open_entry(&self, path: &Path, key: &Key, kind: SourceKind) -> Result<Option<Entry>, Rejected> {
        let file = match open_private(path) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            // A symbolic link fails here (ELOOP).
            Err(_) => return Err(Rejected::NotPrivate),
        };
        let meta = file.metadata().map_err(|_| Rejected::Io)?;
        if !meta.file_type().is_file() || meta.uid() != euid() || meta.mode() & 0o177 != 0 {
            return Err(Rejected::NotPrivate);
        }
        if meta.len() < TRAILER_LEN as u64 {
            return Err(Rejected::Truncated);
        }
        if meta.len() > MAX_ENTRY_LEN {
            return Err(Rejected::Layout);
        }
        let len = meta.len() as usize;
        // Validated through a mapping of its own: reading the whole file for
        // the checksum leaves every page of it resident in the mapping read
        // through, and of the mappings that are kept a load touches a part.
        let whole = Map::new(&file, 0, len).map_err(|_| Rejected::Io)?;
        let b = whole.bytes();
        let t = &b[len - TRAILER_LEN..];
        if t[..8] != MAGIC || u32_at(t, OFF_FORMAT) != FORMAT {
            return Err(Rejected::Format);
        }
        if u64_at(t, OFF_VERSION) != self.version {
            return Err(Rejected::Version);
        }
        let flags = u32_at(t, OFF_FLAGS);
        if t[OFF_KEY..OFF_KEY + 16] != key.0 || flags & 0xff != kind as u32 {
            return Err(Rejected::Key);
        }
        let file_len = u64_at(t, OFF_FILE_LEN);
        if file_len > len as u64 {
            return Err(Rejected::Truncated);
        }
        if file_len != len as u64 {
            return Err(Rejected::Layout);
        }
        let sum_at = len - TRAILER_LEN + OFF_CHECKSUM;
        let mut sum = Xxh3::new();
        sum.update(&b[..sum_at]);
        sum.update(&b[sum_at + 8..]);
        if sum.digest() != u64_at(t, OFF_CHECKSUM) {
            return Err(Rejected::Checksum);
        }
        let body = (len - TRAILER_LEN) as u64;
        let section = |offset: u64, length: u64| -> Result<(usize, usize), Rejected> {
            let end = offset.checked_add(length).ok_or(Rejected::Layout)?;
            if offset % ALIGN as u64 != 0 || end > body {
                return Err(Rejected::Layout);
            }
            Ok((offset as usize, length as usize))
        };
        let (_, bytecode_len) = section(0, u64_at(t, OFF_BYTECODE_LEN))?;
        let source = section(u64_at(t, OFF_SOURCE), u64_at(t, OFF_SOURCE + 8))?;
        let record = section(u64_at(t, OFF_RECORD), u64_at(t, OFF_RECORD + 8))?;
        let text_is_8bit = flags & FLAG_SOURCE_8BIT != 0;
        if bytecode_len == 0 || source.1 == 0 || (!text_is_8bit && source.1 % 2 != 0) {
            return Err(Rejected::Layout);
        }
        let record_bytes: Box<[u8]> = b[record.0..record.0 + record.1].into();
        let source_hash = u32_at(t, OFF_SOURCE_HASH);
        drop(whole);

        let map_offset = source.0 as u64 & !(page_size() - 1);
        let text_offset = source.0 - map_offset as usize;
        let text = Map::new(&file, map_offset, text_offset + source.1).map_err(|_| Rejected::Io)?;
        Ok(Some(Entry {
            path: path.to_path_buf(),
            identity: Identity::of(&meta),
            file: Mutex::new(Some(file)),
            text,
            text_offset,
            text_bytes: source.1,
            text_is_8bit,
            source_hash,
            bytecode_len,
            record: record_bytes,
        }))
    }

    /// Claims the writing of the entry for `key`. `None` when the entry
    /// exists, or when another writer (of any process) has claimed it and is
    /// alive: making an entry means compiling the whole source, which one of
    /// them should do.
    ///
    /// The claim is the temporary file, created exclusively and locked
    /// (`flock`) for as long as its writer lives: what a killed writer left
    /// can be locked, and is then removed and claimed anew.
    pub fn begin(&self, key: &Key) -> Option<Pending<'_>> {
        if self.entry_path(key).exists() {
            return None;
        }
        self.tidied.call_once(|| {
            if let (Some(root), Some(name)) = (self.dir.parent(), self.dir.file_name().and_then(|n| n.to_str())) {
                remove_stale(root, name);
            }
        });
        let temp = self.temp_path(key);
        for _ in 0..2 {
            let created = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temp);
            match created {
                // Whoever holds the lock of the file under this name writes
                // it: another process may have taken this one for abandoned
                // before it was locked.
                Ok(file) => {
                    return (lock(&file) && is_at(&file, &temp)).then(|| Pending {
                        store: self,
                        key: *key,
                        temp,
                        file: Some(file),
                    });
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if !remove_abandoned(&temp) {
                        return None;
                    }
                }
                Err(_) => {
                    self.counters.write_errors.fetch_add(1, Ordering::Relaxed);
                    return None;
                }
            }
        }
        None
    }

    /// The counters so far.
    pub fn stats(&self) -> StoreStats {
        let c = &self.counters;
        StoreStats {
            hits: c.hits.load(Ordering::Relaxed),
            mapped_hits: c.mapped_hits.load(Ordering::Relaxed),
            misses: c.misses.load(Ordering::Relaxed),
            rejected: c.rejected.load(Ordering::Relaxed),
            written: c.written.load(Ordering::Relaxed),
            write_errors: c.write_errors.load(Ordering::Relaxed),
        }
    }
}

/// An entry being written: a temporary file that becomes the entry at
/// [`Pending::commit`] and is removed if dropped before.
pub struct Pending<'a> {
    store: &'a Store,
    key: Key,
    temp: PathBuf,
    file: Option<fs::File>,
}

impl Pending<'_> {
    /// Writes the entry and gives it its name: a reader sees the whole entry
    /// or none.
    pub fn commit(mut self, kind: SourceKind, parts: &EntryParts<'_>) -> io::Result<()> {
        let result = self.write(kind, parts);
        let counter = if result.is_ok() { &self.store.counters.written } else { &self.store.counters.write_errors };
        counter.fetch_add(1, Ordering::Relaxed);
        result
    }

    fn write(&mut self, kind: SourceKind, parts: &EntryParts<'_>) -> io::Result<()> {
        let file = self.file.as_ref().expect("written once");
        let mut out = io::BufWriter::with_capacity(1 << 16, file);
        let mut sum = Xxh3::new();
        let mut at = 0usize;
        let mut put = |out: &mut io::BufWriter<&fs::File>, bytes: &[u8]| -> io::Result<(u64, u64)> {
            let start = at;
            sum.update(bytes);
            out.write_all(bytes)?;
            at += bytes.len();
            let pad = at.next_multiple_of(ALIGN) - at;
            sum.update(&[0u8; ALIGN][..pad]);
            out.write_all(&[0u8; ALIGN][..pad])?;
            at += pad;
            Ok((start as u64, bytes.len() as u64))
        };
        let mut flags = kind as u32;
        if matches!(parts.source, SourceText::Latin1(_)) {
            flags |= FLAG_SOURCE_8BIT;
        }
        let (_, bytecode_len) = put(&mut out, parts.bytecode)?;
        let source = put(&mut out, parts.source.bytes())?;
        let record = put(&mut out, parts.record)?;

        let mut t = [0u8; TRAILER_LEN];
        t[..8].copy_from_slice(&MAGIC);
        t[OFF_FORMAT..OFF_FORMAT + 4].copy_from_slice(&FORMAT.to_le_bytes());
        t[OFF_FLAGS..OFF_FLAGS + 4].copy_from_slice(&flags.to_le_bytes());
        t[OFF_KEY..OFF_KEY + 16].copy_from_slice(&self.key.0);
        t[OFF_VERSION..OFF_VERSION + 8].copy_from_slice(&self.store.version.to_le_bytes());
        t[OFF_FILE_LEN..OFF_FILE_LEN + 8].copy_from_slice(&((at + TRAILER_LEN) as u64).to_le_bytes());
        t[OFF_SOURCE_HASH..OFF_SOURCE_HASH + 4].copy_from_slice(&parts.source_hash.to_le_bytes());
        t[OFF_BYTECODE_LEN..OFF_BYTECODE_LEN + 8].copy_from_slice(&bytecode_len.to_le_bytes());
        for (at, (offset, length)) in [(OFF_SOURCE, source), (OFF_RECORD, record)] {
            t[at..at + 8].copy_from_slice(&offset.to_le_bytes());
            t[at + 8..at + 16].copy_from_slice(&length.to_le_bytes());
        }
        sum.update(&t[..OFF_CHECKSUM]);
        sum.update(&t[OFF_CHECKSUM + 8..]);
        let digest = sum.digest();
        t[OFF_CHECKSUM..OFF_CHECKSUM + 8].copy_from_slice(&digest.to_le_bytes());
        out.write_all(&t)?;
        out.flush()?;
        drop(out);
        // Renamed under the lock: the name is still this writer's file.
        fs::rename(&self.temp, self.store.entry_path(&self.key))?;
        self.file = None;
        Ok(())
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        // Not committed: the claim is given up, under its lock.
        if self.file.is_some() {
            let _ = fs::remove_file(&self.temp);
        }
    }
}

/// Takes the lock of a writer's temporary file, if nobody holds it.
fn lock(file: &fs::File) -> bool {
    // SAFETY: a descriptor this process owns; does not block.
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}

/// Whether `path` names `file` (and not a file that took its place).
fn is_at(file: &fs::File, path: &Path) -> bool {
    match (file.metadata(), fs::symlink_metadata(path)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// Removes the temporary file at `path` if its writer is gone (its lock is
/// free). `false` while a writer holds it, and when it cannot be removed.
fn remove_abandoned(path: &Path) -> bool {
    let Ok(file) = fs::OpenOptions::new().write(true).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)
    else {
        // Renamed or removed since it was seen: nothing left to remove.
        return !path.exists();
    };
    // Under the lock nobody else removes or renames it.
    lock(&file) && is_at(&file, path) && fs::remove_file(path).is_ok()
}

/// Removes the version directories beside `current` that this user owns and
/// nobody has opened for [`STALE_VERSION_AGE`], and temporary files a killed
/// writer left in `current` (old ones nobody holds the lock of).
fn remove_stale(root: &Path, current: &str) {
    let now = SystemTime::now();
    let stale = |meta: &fs::Metadata, age: Duration| {
        meta.uid() == euid() && meta.modified().ok().and_then(|m| now.duration_since(m).ok()).is_some_and(|d| d > age)
    };
    for entry in fs::read_dir(root).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let is_version = name.len() == 18 && name.starts_with("v-") && name[2..].bytes().all(|b| b.is_ascii_hexdigit());
        if !is_version || name == current {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if meta.file_type().is_dir() && stale(&meta, STALE_VERSION_AGE) {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
    for entry in fs::read_dir(root.join(current)).into_iter().flatten().flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.starts_with('.') && n.ends_with(".tmp")) {
            continue;
        }
        let Ok(meta) = fs::symlink_metadata(entry.path()) else { continue };
        if meta.file_type().is_file() && stale(&meta, STALE_TEMP_AGE) {
            remove_abandoned(&entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let dir = std::env::temp_dir().join(format!("pyro-bytecode-store-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Dir(dir)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn write(store: &Store, key: &Key, text: &str) {
        let units: Vec<u16>;
        let source = if text.is_ascii() {
            SourceText::Latin1(text.as_bytes())
        } else {
            units = text.encode_utf16().collect();
            SourceText::Utf16(&units)
        };
        let parts = EntryParts { source, source_hash: 7, bytecode: b"not bytecode", record: b"" };
        store.begin(key).expect("claimed").commit(SourceKind::Classic, &parts).expect("written");
    }

    /// Two texts under one key (what a collision of the key's hash is): the
    /// entry is given to its own text only, and stays.
    #[test]
    fn an_entry_is_given_to_its_own_text_only() {
        for (name, text, other) in [
            ("text-ascii", "var a = 1;", "var a = 2;"),
            ("text-utf16", "var a = '\u{2192}';", "var a = '\u{2190}';"),
            ("text-width", "var a = 'e';", "var a = '\u{e9}';"),
        ] {
            let dir = Dir::new(name);
            let key = Key([7; 16]);
            let store = Store::open(&dir.0, 1).unwrap();
            write(&store, &key, text);
            // Opened anew, and from the entries the store holds mapped.
            for store in [Store::open(&dir.0, 1).unwrap(), Store::open(&dir.0, 1).unwrap()] {
                assert!(matches!(store.lookup(&key, SourceKind::Classic, &[other]), Lookup::OtherText), "{name}");
                let Lookup::Hit(entry) = store.lookup(&key, SourceKind::Classic, &[text]) else { panic!("{name}: no hit") };
                assert!(matches!(store.lookup(&key, SourceKind::Classic, &[other]), Lookup::OtherText), "{name}");
                // A text in parts is the parts in a row.
                let (head, tail) = text.split_at(4);
                assert!(entry.has_text(&[head, "", tail]), "{name}");
                assert!(!entry.has_text(&[head]), "{name}");
                assert!(!entry.has_text(&[text, "x"]), "{name}");
                assert!(!entry.has_text(&[tail, head]), "{name}");
                assert!(store.entry_path(&key).exists(), "{name}");
                assert_eq!(store.stats().rejected, 0, "{name}");
            }
        }
    }

    #[test]
    fn a_key_is_of_the_kind_the_host_and_the_whole_text() {
        let key = |kind, host: &str, parts: &[&str]| Key::new(kind, host, parts);
        let base = key(SourceKind::Classic, "a.test", &["var a", " = 1;"]);
        assert_eq!(base, key(SourceKind::Classic, "a.test", &["var a = 1;"]));
        assert_ne!(base, key(SourceKind::Module, "a.test", &["var a = 1;"]));
        assert_ne!(base, key(SourceKind::Classic, "b.test", &["var a = 1;"]));
        assert_ne!(base, key(SourceKind::Classic, "a.test", &["var a = 2;"]));
        // The host and the text do not run into each other.
        assert_ne!(key(SourceKind::Classic, "a.test", &["v"]), key(SourceKind::Classic, "a.testv", &[""]));
    }
}
