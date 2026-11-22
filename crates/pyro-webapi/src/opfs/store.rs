//! The origin-private file system of a browser context. Handles are locators
//! (root, kind, path), and never contain or resolve a host file-system path.
//! A single origin lock makes lookup, mutation and file-lease acquisition atomic.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

/// Pyro's RAM-backed context storage budgets both committed contents and
/// outstanding transactional write buffers. A sparse extension is charged
/// for its zero-filled bytes before memory is allocated.
pub(crate) const QUOTA: u64 = 64 * 1024 * 1024;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64
}

type Path = Vec<String>;
type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Error {
    InvalidName,
    Missing,
    WrongKind,
    Locked,
    InvalidModification,
    Closed,
    Quota,
    Unknown,
}

impl Error {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::InvalidName => "TypeError",
            Self::Missing => "NotFoundError",
            Self::WrongKind => "TypeMismatchError",
            Self::Locked => "NoModificationAllowedError",
            Self::InvalidModification => "InvalidModificationError",
            Self::Closed => "InvalidStateError",
            Self::Quota => "QuotaExceededError",
            Self::Unknown => "UnknownError",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    File,
    Directory,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    ReadWrite,
    ReadOnly,
    ReadWriteUnsafe,
}

/// A shared lock can coexist only with other holders of that same mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LockMode {
    Siloed,
    Exclusive,
    Sync(Mode),
}

impl LockMode {
    fn shared(self) -> bool {
        matches!(self, Self::Siloed | Self::Sync(Mode::ReadOnly | Mode::ReadWriteUnsafe))
    }
}

struct File {
    bytes: Vec<u8>,
    mtime: i64,
    lock: Option<(LockMode, usize)>,
}

impl File {
    fn acquire(&mut self, mode: LockMode) -> Result<()> {
        match &mut self.lock {
            None => self.lock = Some((mode, 1)),
            Some((current, count)) if *current == mode && mode.shared() => *count += 1,
            Some(_) => return Err(Error::Locked),
        }
        Ok(())
    }

    fn release(&mut self) {
        match &mut self.lock {
            Some((_, 1)) => self.lock = None,
            Some((_, count)) => *count -= 1,
            None => unreachable!("each live lease holds a file lock"),
        }
    }
}

enum Data {
    File(File),
    Directory(BTreeSet<String>),
}

struct Entry {
    data: Data,
    uuid: Option<String>,
}

impl Entry {
    fn new(kind: Kind) -> Self {
        let data = match kind {
            Kind::Directory => Data::Directory(BTreeSet::new()),
            Kind::File => Data::File(File { bytes: Vec::new(), mtime: now_ms(), lock: None }),
        };
        Self { data, uuid: None }
    }

    fn kind(&self) -> Kind {
        match self.data {
            Data::File(_) => Kind::File,
            Data::Directory(_) => Kind::Directory,
        }
    }
}

struct Tree {
    entries: BTreeMap<Path, Entry>,
    used: u64,
    staged: u64,
}

impl Default for Tree {
    fn default() -> Self {
        Self { entries: BTreeMap::from([(Path::new(), Entry::new(Kind::Directory))]), used: 0, staged: 0 }
    }
}

impl Tree {
    fn file_mut(&mut self, path: &[String]) -> Result<&mut File> {
        match &mut self.entries.get_mut(path).ok_or(Error::Missing)?.data {
            Data::File(file) => Ok(file),
            Data::Directory(_) => Err(Error::Missing),
        }
    }

    fn dir_mut(&mut self, path: &[String]) -> Result<&mut BTreeSet<String>> {
        match &mut self.entries.get_mut(path).ok_or(Error::Missing)?.data {
            Data::Directory(children) => Ok(children),
            Data::File(_) => Err(Error::Missing),
        }
    }

    fn check_growth(&self, old_len: usize, new_len: u64) -> Result<usize> {
        if new_len.saturating_sub(old_len as u64) > QUOTA - self.used - self.staged {
            return Err(Error::Quota);
        }
        usize::try_from(new_len).map_err(|_| Error::Quota)
    }

    fn remove(&mut self, path: Path, recursive: bool) -> Result<()> {
        let entry = self.entries.get(&path).ok_or(Error::Missing)?;
        let mut non_empty = false;
        if let Data::Directory(children) = &entry.data {
            non_empty = !children.is_empty();
        }
        // Check every lock before removing any member of the subtree.
        let mut deleted = Vec::new();
        let mut freed = 0;
        for (key, e) in self.entries.range(path.clone()..).take_while(|(key, _)| key.starts_with(&path)) {
            if let Data::File(file) = &e.data {
                if file.lock.is_some() {
                    return Err(Error::Locked);
                }
                freed += file.bytes.len() as u64;
            }
            deleted.push(key.clone());
        }
        if !path.is_empty() && non_empty && !recursive {
            return Err(Error::InvalidModification);
        }
        for key in deleted {
            self.entries.remove(&key);
        }
        self.used -= freed;
        if let Some((name, parent)) = path.split_last() {
            self.dir_mut(parent)?.remove(name);
        } else {
            // Removing the bucket root clears the origin's private file system.
            self.entries.insert(Path::new(), Entry::new(Kind::Directory));
        }
        Ok(())
    }
}

#[derive(Clone, Default)]
pub(crate) struct Namespace {
    origins: Arc<Mutex<HashMap<String, Arc<Mutex<Tree>>>>>,
}

impl Namespace {
    pub(crate) fn root(&self, origin: &str) -> Handle {
        let tree = lock(&self.origins).entry(origin.to_owned()).or_default().clone();
        Handle::new(tree, Path::new(), Kind::Directory)
    }

    pub(crate) fn usage(&self, origin: &str) -> u64 {
        lock(&self.origins).get(origin).map_or(0, |tree| lock(tree).used)
    }
}

fn validate_name(name: &str) -> Result<()> {
    if matches!(name, "" | "." | "..") || name.contains(['/', '\\', '\0']) {
        return Err(Error::InvalidName);
    }
    Ok(())
}

/// Clones of a handle share the locator so a move updates references to
/// that handle. A separately obtained handle still addresses its own path.
#[derive(Clone)]
pub(crate) struct Handle {
    tree: Arc<Mutex<Tree>>,
    path: Arc<Mutex<Path>>,
    kind: Kind,
}

impl Handle {
    fn new(tree: Arc<Mutex<Tree>>, path: Path, kind: Kind) -> Self {
        Self { tree, path: Arc::new(Mutex::new(path)), kind }
    }

    pub(crate) fn kind(&self) -> Kind {
        self.kind
    }

    pub(crate) fn name(&self) -> String {
        lock(&self.path).last().cloned().unwrap_or_default()
    }

    pub(crate) fn unique_id(&self) -> Result<String> {
        let mut tree = lock(&self.tree);
        let path = lock(&self.path);
        let entry = tree.entries.get_mut(&*path).filter(|e| e.kind() == self.kind).ok_or(Error::Missing)?;
        if let Some(id) = &entry.uuid {
            return Ok(id.clone());
        }
        let id = crate::crypto::random_uuid().map_err(|_| Error::Unknown)?;
        entry.uuid = Some(id.clone());
        Ok(id)
    }

    pub(crate) fn is_same_entry(&self, other: &Self) -> bool {
        if !Arc::ptr_eq(&self.tree, &other.tree) || self.kind != other.kind {
            return false;
        }
        if Arc::ptr_eq(&self.path, &other.path) {
            return true;
        }
        let _tree = lock(&self.tree);
        *lock(&self.path) == *lock(&other.path)
    }

    pub(crate) fn get(&self, name: &str, kind: Kind, create: bool) -> Result<Self> {
        validate_name(name)?;
        let mut tree = lock(&self.tree);
        let mut path = lock(&self.path).clone();
        if self.kind != Kind::Directory {
            return Err(Error::WrongKind);
        }
        tree.dir_mut(&path)?;
        path.push(name.to_owned());
        if let Some(entry) = tree.entries.get(&path) {
            if entry.kind() != kind {
                return Err(Error::WrongKind);
            }
        } else if create {
            tree.dir_mut(&path[..path.len() - 1])?.insert(name.to_owned());
            tree.entries.insert(path.clone(), Entry::new(kind));
        } else {
            return Err(Error::Missing);
        }
        Ok(Self::new(self.tree.clone(), path, kind))
    }

    pub(crate) fn next(&self, after: Option<&str>) -> Result<Option<(String, Self)>> {
        let mut tree = lock(&self.tree);
        let mut path = lock(&self.path).clone();
        if self.kind != Kind::Directory {
            return Err(Error::WrongKind);
        }
        let children = tree.dir_mut(&path)?;
        let name = match after {
            Some(cursor) => children.range::<str, _>((Excluded(cursor), Unbounded)).next(),
            None => children.first(),
        };
        let Some(name) = name.cloned() else {
            return Ok(None);
        };
        path.push(name.clone());
        let kind = tree.entries[&path].kind();
        Ok(Some((name, Self::new(self.tree.clone(), path, kind))))
    }

    pub(crate) fn resolve(&self, other: &Self) -> Result<Option<Vec<String>>> {
        if self.kind != Kind::Directory {
            return Err(Error::WrongKind);
        }
        if !Arc::ptr_eq(&self.tree, &other.tree) {
            return Ok(None);
        }
        if Arc::ptr_eq(&self.path, &other.path) {
            return Ok(Some(Vec::new()));
        }
        let _tree = lock(&self.tree);
        let parent = lock(&self.path);
        let child = lock(&other.path);
        Ok(child.strip_prefix(parent.as_slice()).map(|rest| rest.to_vec()))
    }

    pub(crate) fn remove_entry(&self, name: &str, recursive: bool) -> Result<()> {
        validate_name(name)?;
        if self.kind != Kind::Directory {
            return Err(Error::WrongKind);
        }
        let mut tree = lock(&self.tree);
        let mut path = lock(&self.path).clone();
        tree.dir_mut(&path)?;
        path.push(name.to_owned());
        tree.remove(path, recursive)
    }

    pub(crate) fn remove(&self, recursive: bool) -> Result<()> {
        let mut tree = lock(&self.tree);
        let path = lock(&self.path).clone();
        if tree.entries.get(&path).is_none_or(|entry| entry.kind() != self.kind) {
            return Err(Error::Missing);
        }
        tree.remove(path, recursive)
    }

    pub(crate) fn move_to(&self, directory: Option<&Self>, new_name: Option<&str>) -> Result<()> {
        if let Some(name) = new_name {
            validate_name(name)?;
        }
        if self.kind != Kind::File {
            return Err(Error::WrongKind);
        }
        if directory.is_some_and(|dir| dir.kind != Kind::Directory || !Arc::ptr_eq(&dir.tree, &self.tree)) {
            return Err(Error::InvalidModification);
        }
        let mut tree = lock(&self.tree);
        let mut source = lock(&self.path);
        if tree.file_mut(&source)?.lock.is_some() {
            return Err(Error::Locked);
        }
        let (old_name, old_parent) = source.split_last().ok_or(Error::InvalidModification)?;
        let parent = directory.map_or_else(|| old_parent.to_vec(), |dir| lock(&dir.path).clone());
        tree.dir_mut(&parent)?;
        let name = new_name.unwrap_or(old_name).to_owned();
        let mut target = parent.clone();
        target.push(name.clone());
        if *source == target {
            return Ok(());
        }
        if let Some(other) = tree.entries.get(&target) {
            match &other.data {
                Data::Directory(_) => return Err(Error::InvalidModification),
                Data::File(file) if file.lock.is_some() => return Err(Error::Locked),
                Data::File(file) => tree.used -= file.bytes.len() as u64,
            }
        }
        tree.dir_mut(old_parent)?.remove(old_name);
        tree.dir_mut(&parent)?.insert(name);
        let file = tree.entries.remove(&*source).ok_or(Error::Missing)?;
        tree.entries.insert(target.clone(), file);
        *source = target;
        Ok(())
    }

    pub(crate) fn snapshot(&self) -> Result<(Vec<u8>, i64)> {
        if self.kind != Kind::File {
            return Err(Error::WrongKind);
        }
        let mut tree = lock(&self.tree);
        let file = tree.file_mut(&lock(&self.path))?;
        Ok((file.bytes.clone(), file.mtime))
    }

    pub(crate) fn writable(&self, keep: bool, exclusive: bool) -> Result<Lease> {
        let mode = if exclusive { LockMode::Exclusive } else { LockMode::Siloed };
        self.open(mode, Some(keep))
    }

    pub(crate) fn sync(&self, mode: Mode) -> Result<Lease> {
        self.open(LockMode::Sync(mode), None)
    }

    fn open(&self, mode: LockMode, keep: Option<bool>) -> Result<Lease> {
        if self.kind != Kind::File {
            return Err(Error::WrongKind);
        }
        let mut tree = lock(&self.tree);
        let path = lock(&self.path).clone();
        let file = tree.file_mut(&path)?;
        let size = if keep == Some(true) { file.bytes.len() } else { 0 };
        tree.check_growth(0, size as u64)?;
        let file = tree.file_mut(&path)?;
        file.acquire(mode)?;
        let pending = keep.map(|copy| if copy { file.bytes.clone() } else { Vec::new() });
        tree.staged += size as u64;
        Ok(Lease { tree: self.tree.clone(), path, mode, pending, cursor: 0, closed: false })
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidName => "The name is not a valid file system entry name.",
            Self::Missing => "The requested file system entry does not exist.",
            Self::WrongKind => "An entry of a different kind exists at the requested location.",
            Self::Locked => "The operation is not allowed by the file's current access mode or locks.",
            Self::InvalidModification => "The requested change to the file system is not permitted.",
            Self::Closed => "The file access handle is closed.",
            Self::Quota => "The operation would exceed the origin's file storage quota.",
            Self::Unknown => "The file system could not allocate an entry identifier.",
        })
    }
}

/// A writable stream keeps its staged bytes private until close. A synchronous
/// handle reads and modifies the live file under the same origin lock. Its
/// cursor belongs to this handle, including for shared read/write modes.
pub(crate) struct Lease {
    tree: Arc<Mutex<Tree>>,
    path: Path,
    mode: LockMode,
    pending: Option<Vec<u8>>,
    cursor: u64,
    closed: bool,
}

impl Lease {
    fn active(&self) -> Result<()> {
        if self.closed { Err(Error::Closed) } else { Ok(()) }
    }

    fn may_write(&self) -> Result<()> {
        self.active()?;
        if self.mode == LockMode::Sync(Mode::ReadOnly) { Err(Error::Locked) } else { Ok(()) }
    }

    pub(crate) fn size(&self) -> Result<u64> {
        self.active()?;
        if let Some(buffer) = &self.pending {
            return Ok(buffer.len() as u64);
        }
        Ok(lock(&self.tree).file_mut(&self.path)?.bytes.len() as u64)
    }

    pub(crate) fn read_bytes(&mut self, length: usize, at: Option<u64>) -> Result<Vec<u8>> {
        self.active()?;
        let start = at.unwrap_or(self.cursor);
        let mut tree = lock(&self.tree);
        let bytes = match &self.pending {
            Some(buffer) => buffer.as_slice(),
            None => tree.file_mut(&self.path)?.bytes.as_slice(),
        };
        let index = start.min(bytes.len() as u64) as usize;
        let count = length.min(bytes.len() - index);
        // Reading beyond EOF returns zero and resets the cursor to EOF.
        self.cursor = (index + count) as u64;
        Ok(bytes[index..index + count].to_vec())
    }

    pub(crate) fn write(&mut self, bytes: &[u8], at: Option<u64>) -> Result<usize> {
        self.may_write()?;
        let start = at.unwrap_or(self.cursor);
        let end = start.checked_add(bytes.len() as u64).ok_or(Error::Quota)?;
        self.modify(end, Some(bytes))?;
        self.cursor = end;
        Ok(bytes.len())
    }

    pub(crate) fn truncate(&mut self, size: u64) -> Result<()> {
        self.may_write()?;
        self.modify(size, None)?;
        self.cursor = self.cursor.min(size);
        Ok(())
    }

    /// When writing a chunk, keep any bytes following it. A truncate instead
    /// sets the exact file size. Charge the corresponding live or staged size.
    fn modify(&mut self, end: u64, chunk: Option<&[u8]>) -> Result<()> {
        let mut tree = lock(&self.tree);
        let previous = match &self.pending {
            Some(buffer) => buffer.len(),
            None => tree.file_mut(&self.path)?.bytes.len(),
        };
        let requested = if chunk.is_some() { end.max(previous as u64) } else { end };
        let length = tree.check_growth(previous, requested)?;
        let bytes = match &mut self.pending {
            Some(buffer) => buffer,
            None => &mut tree.file_mut(&self.path)?.bytes,
        };
        if length > bytes.len() {
            bytes.try_reserve_exact(length - bytes.len()).map_err(|_| Error::Quota)?;
            bytes.resize(length, 0);
        } else if length < bytes.len() {
            bytes.truncate(length);
            if length <= bytes.capacity() / 2 {
                bytes.shrink_to_fit();
            }
        }
        if let Some(chunk) = chunk {
            let end = end as usize;
            bytes[end - chunk.len()..end].copy_from_slice(chunk);
        }
        if self.pending.is_some() {
            tree.staged = tree.staged - previous as u64 + length as u64;
        } else {
            tree.used = tree.used - previous as u64 + length as u64;
            tree.file_mut(&self.path)?.mtime = now_ms();
        }
        Ok(())
    }

    pub(crate) fn seek(&mut self, at: u64) -> Result<()> {
        self.active()?;
        self.cursor = at;
        Ok(())
    }

    pub(crate) fn flush(&mut self) -> Result<()> {
        self.may_write()
    }

    pub(crate) fn close(&mut self) -> Result<()> {
        self.finish(true)
    }

    pub(crate) fn abort(&mut self) -> Result<()> {
        self.finish(false)
    }

    fn finish(&mut self, commit: bool) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        let mut tree = lock(&self.tree);
        let file = tree.file_mut(&self.path)?;
        file.release();
        if let Some(buffer) = self.pending.take() {
            let next = buffer.len() as u64;
            if commit {
                let old = file.bytes.len() as u64;
                file.bytes = buffer;
                file.mtime = now_ms();
                tree.used = tree.used - old + next;
            }
            tree.staged -= next;
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = self.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_roots_and_locators_follow_context_lifetime() {
        let storage = Namespace::default();
        let root = storage.root("https://example.org");
        let dir = root.get("dir", Kind::Directory, true).unwrap();
        let file = dir.get("doc", Kind::File, true).unwrap();
        let locator = dir.get("doc", Kind::File, false).unwrap();
        let old_id = file.unique_id().unwrap();
        assert!(file.is_same_entry(&locator));
        assert!(root.is_same_entry(&storage.clone().root("https://example.org")));
        assert!(!root.is_same_entry(&storage.root("https://other.org")));
        assert!(!root.is_same_entry(&Namespace::default().root("https://example.org")));
        assert_eq!(root.resolve(&file).unwrap().unwrap(), ["dir", "doc"]);
        for name in ["", ".", "..", "dir/doc", "dir\\doc"] {
            assert!(matches!(dir.get(name, Kind::File, true), Err(Error::InvalidName)));
        }
        file.remove(false).unwrap();
        assert!(matches!(locator.snapshot(), Err(Error::Missing)));
        let replacement = dir.get("doc", Kind::File, true).unwrap();
        assert!(replacement.is_same_entry(&locator));
        assert_ne!(replacement.unique_id().unwrap(), old_id);
        replacement.move_to(Some(&root), Some("moved")).unwrap();
        assert_eq!(root.resolve(&replacement).unwrap().unwrap(), ["moved"]);
        assert!(matches!(locator.snapshot(), Err(Error::Missing)));
        assert_eq!(root.next(Some("dir")).unwrap().unwrap().0, "moved");
    }

    #[test]
    fn siloed_streams_commit_atomically_and_abandonment_releases_locks() {
        let storage = Namespace::default();
        let root = storage.root("origin");
        let file = root.get("file", Kind::File, true).unwrap();
        let mut first = file.writable(false, false).unwrap();
        let mut second = file.writable(false, false).unwrap();
        first.write(b"abc", Some(2)).unwrap();
        second.write(b"later", None).unwrap();
        assert!(file.snapshot().unwrap().0.is_empty());
        assert_eq!(root.remove(true), Err(Error::Locked));
        assert!(matches!(file.sync(Mode::ReadOnly), Err(Error::Locked)));
        first.close().unwrap();
        assert_eq!(file.snapshot().unwrap().0, b"\0\0abc");
        second.close().unwrap();
        assert_eq!(file.snapshot().unwrap().0, b"later");
        let mut abandoned = file.writable(true, true).unwrap();
        abandoned.truncate(1).unwrap();
        assert_eq!(abandoned.write(b"x", Some(QUOTA)), Err(Error::Quota));
        drop(abandoned);
        assert_eq!(file.snapshot().unwrap().0, b"later");
        assert_eq!(storage.usage("origin"), 5);
        let copy = file.snapshot().unwrap().0;
        file.writable(false, true).unwrap().close().unwrap();
        assert_eq!(storage.usage("origin"), 0);
        assert_eq!(copy, b"later");
    }

    #[test]
    fn sync_modes_and_cursors_are_per_handle() {
        let root = Namespace::default().root("origin");
        let file = root.get("file", Kind::File, true).unwrap();
        let mut a = file.sync(Mode::ReadWriteUnsafe).unwrap();
        let mut b = file.sync(Mode::ReadWriteUnsafe).unwrap();
        a.write(b"abcdef", None).unwrap();
        assert_eq!(b.read_bytes(2, Some(1)).unwrap(), b"bc");
        b.write(b"X", None).unwrap();
        assert_eq!(a.read_bytes(10, Some(0)).unwrap(), b"abcXef");
        a.truncate(3).unwrap();
        a.write(b"Y", None).unwrap();
        assert_eq!(file.snapshot().unwrap().0, b"abcY");
        assert_eq!(a.read_bytes(1, Some(4096)).unwrap(), b"");
        a.write(b"!", None).unwrap();
        assert_eq!(file.snapshot().unwrap().0, b"abcY!");
        assert!(matches!(file.sync(Mode::ReadOnly), Err(Error::Locked)));
        assert!(matches!(file.writable(false, false), Err(Error::Locked)));
        drop(a);
        b.close().unwrap();
        b.close().unwrap();
        assert_eq!(b.size(), Err(Error::Closed));
        let mut ro = file.sync(Mode::ReadOnly).unwrap();
        let second = file.sync(Mode::ReadOnly).unwrap();
        assert_eq!(ro.read_bytes(99, None).unwrap(), b"abcY!");
        assert_eq!(ro.truncate(0), Err(Error::Locked));
        assert_eq!(ro.flush(), Err(Error::Locked));
        assert_eq!(ro.write(b"!", None), Err(Error::Locked));
        drop(second);
        drop(ro);
        file.sync(Mode::ReadWrite).unwrap();
    }
}
