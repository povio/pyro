//! The origin-private file system of a browser context. Handles are locators
//! (root, kind, path), and never contain or resolve a host file-system path.
//! A single origin lock makes lookup, mutation and file-lease acquisition atomic.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::storage_buckets::catalog::{Endpoint, Error as BucketError, Lease as BucketLease, Reservation};

impl From<BucketError> for Error {
    fn from(error: BucketError) -> Self {
        match error {
            BucketError::Removed => Error::Missing,
            BucketError::QuotaExceeded => Error::Quota,
            _ => Error::Unknown,
        }
    }
}

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
    bucket: Option<BucketLease>,
    entries: BTreeMap<Path, Entry>,
    /// A move reserves both locators. A removal reserves the whole subtree.
    reserved: BTreeSet<Path>,
    used: u64,
    staged: u64,
}

impl Default for Tree {
    fn default() -> Self {
        Self {
            bucket: None,
            entries: BTreeMap::from([(Path::new(), Entry::new(Kind::Directory))]),
            reserved: BTreeSet::new(),
            used: 0,
            staged: 0,
        }
    }
}

impl Tree {
    fn active(&self) -> Result<()> {
        if let Some(bucket) = &self.bucket {
            bucket.check()?;
        }
        Ok(())
    }

    fn charge(&self, removed: u64, added: u64) -> Result<()> {
        if let Some(bucket) = &self.bucket {
            bucket.charge(Endpoint::FileSystem, removed, added)?;
        }
        Ok(())
    }

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

    fn check_reservation(&self, path: &[String]) -> Result<()> {
        if self.reserved.is_empty() {
            return Ok(());
        }
        let ancestor = (0..path.len()).any(|len| self.reserved.contains(&path[..len]));
        // A path sorts before all of its children. Only the next reservation
        // can be the path itself or the first reservation inside its subtree.
        let descendant = self
            .reserved
            .range::<[String], _>((Included(path), Unbounded))
            .next()
            .is_some_and(|entry| entry.starts_with(path));
        if ancestor || descendant { Err(Error::Locked) } else { Ok(()) }
    }

    fn prepare_remove(&self, path: Path, kind: Option<Kind>, recursive: bool) -> Result<Plan> {
        self.check_reservation(&path)?;
        let entry = self
            .entries
            .get(&path)
            .filter(|entry| kind.is_none_or(|kind| entry.kind() == kind))
            .ok_or(Error::Missing)?;
        let mut deleted = Vec::new();
        for (key, entry) in self.entries.range(path.clone()..).take_while(|(key, _)| key.starts_with(&path)) {
            if matches!(&entry.data, Data::File(file) if file.lock.is_some()) {
                return Err(Error::Locked);
            }
            deleted.push(key.clone());
        }
        if !path.is_empty() && !recursive && matches!(&entry.data, Data::Directory(children) if !children.is_empty()) {
            return Err(Error::InvalidModification);
        }
        Ok(Plan::Remove { path, deleted })
    }
}

#[derive(Clone, Default)]
pub(crate) struct Namespace {
    bucket: Option<BucketLease>,
    origins: Arc<Mutex<HashMap<String, Arc<Mutex<Tree>>>>>,
}

impl Namespace {
    pub(crate) fn for_bucket(bucket: BucketLease) -> Self {
        Self { bucket: Some(bucket), ..Self::default() }
    }

    /// The catalog has already retired the generation and released its own
    /// locks. Drop large committed contents even when script retains locators.
    pub(crate) fn retire(&self) {
        let trees = std::mem::take(&mut *lock(&self.origins));
        for (_, tree) in trees {
            let mut tree = lock(&tree);
            tree.entries.clear();
            tree.reserved.clear();
            tree.used = 0;
        }
    }

    pub(crate) fn root(&self, origin: &str) -> Handle {
        let tree = lock(&self.origins)
            .entry(origin.to_owned())
            .or_insert_with(|| Arc::new(Mutex::new(Tree { bucket: self.bucket.clone(), ..Tree::default() })))
            .clone();
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
        tree.active()?;
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
        tree.active()?;
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
            tree.check_reservation(&path)?;
            tree.dir_mut(&path[..path.len() - 1])?.insert(name.to_owned());
            tree.entries.insert(path.clone(), Entry::new(kind));
        } else {
            return Err(Error::Missing);
        }
        Ok(Self::new(self.tree.clone(), path, kind))
    }

    pub(crate) fn next(&self, after: Option<&str>) -> Result<Option<(String, Self)>> {
        let mut tree = lock(&self.tree);
        tree.active()?;
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
        let tree = lock(&self.tree);
        tree.active()?;
        if !Arc::ptr_eq(&self.tree, &other.tree) {
            return Ok(None);
        }
        if Arc::ptr_eq(&self.path, &other.path) {
            return Ok(Some(Vec::new()));
        }
        let parent = lock(&self.path);
        let child = lock(&other.path);
        Ok(child.strip_prefix(parent.as_slice()).map(|rest| rest.to_vec()))
    }

    pub(crate) fn reserve_remove_entry(&self, name: &str, recursive: bool) -> Result<Mutation> {
        validate_name(name)?;
        if self.kind != Kind::Directory {
            return Err(Error::WrongKind);
        }
        let mut tree = lock(&self.tree);
        tree.active()?;
        let mut path = lock(&self.path).clone();
        tree.dir_mut(&path)?;
        path.push(name.to_owned());
        let plan = tree.prepare_remove(path, None, recursive)?;
        Ok(self.reserve(&mut tree, plan))
    }

    pub(crate) fn reserve_remove(&self, recursive: bool) -> Result<Mutation> {
        let mut tree = lock(&self.tree);
        tree.active()?;
        let path = lock(&self.path).clone();
        let plan = tree.prepare_remove(path, Some(self.kind), recursive)?;
        Ok(self.reserve(&mut tree, plan))
    }

    pub(crate) fn reserve_move(&self, directory: Option<&Self>, new_name: Option<&str>) -> Result<Mutation> {
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
        tree.active()?;
        let source = lock(&self.path).clone();
        tree.check_reservation(&source)?;
        if tree.file_mut(&source)?.lock.is_some() {
            return Err(Error::Locked);
        }
        let (old_name, old_parent) = source.split_last().ok_or(Error::InvalidModification)?;
        let mut target = directory.map_or_else(|| old_parent.to_vec(), |dir| lock(&dir.path).clone());
        tree.dir_mut(&target)?;
        target.push(new_name.unwrap_or(old_name).to_owned());
        tree.check_reservation(&target)?;
        if let Some(entry) = tree.entries.get(&target) {
            match &entry.data {
                Data::Directory(_) => return Err(Error::InvalidModification),
                Data::File(file) if file.lock.is_some() => return Err(Error::Locked),
                Data::File(_) => {}
            }
        }
        let plan = Plan::Move { source, target, locator: self.path.clone() };
        Ok(self.reserve(&mut tree, plan))
    }

    fn reserve(&self, tree: &mut Tree, plan: Plan) -> Mutation {
        // All checks and both exclusive reservations share one origin lock.
        let (first, second) = plan.paths();
        tree.reserved.insert(first.clone());
        if let Some(path) = second {
            tree.reserved.insert(path.clone());
        }
        Mutation { tree: self.tree.clone(), plan: Some(plan) }
    }

    pub(crate) fn snapshot(&self) -> Result<(Vec<u8>, i64)> {
        if self.kind != Kind::File {
            return Err(Error::WrongKind);
        }
        let mut tree = lock(&self.tree);
        tree.active()?;
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
        tree.active()?;
        let path = lock(&self.path).clone();
        tree.check_reservation(&path)?;
        let file = tree.file_mut(&path)?;
        let size = if keep == Some(true) { file.bytes.len() } else { 0 };
        tree.check_growth(0, size as u64)?;
        let reservation = if keep.is_some() {
            tree.bucket.as_ref().map(|bucket| bucket.reserve(Endpoint::FileSystem, 0)).transpose()?
        } else {
            None
        };
        let file = tree.file_mut(&path)?;
        file.acquire(mode)?;
        let pending = keep.map(|copy| if copy { file.bytes.clone() } else { Vec::new() });
        tree.staged += size as u64;
        Ok(Lease { tree: self.tree.clone(), path, mode, pending, reservation, cursor: 0, closed: false })
    }
}

enum Plan {
    Remove { path: Path, deleted: Vec<Path> },
    Move { source: Path, target: Path, locator: Arc<Mutex<Path>> },
}

impl Plan {
    fn paths(&self) -> (&Path, Option<&Path>) {
        match self {
            Self::Remove { path, .. } => (path, None),
            Self::Move { source, target, .. } => (source, (source != target).then_some(target)),
        }
    }

    fn release(&self, tree: &mut Tree) {
        let (first, second) = self.paths();
        tree.reserved.remove(first);
        if let Some(path) = second {
            tree.reserved.remove(path);
        }
    }
}

/// Keeps the selected source and destination unchanged until the queued storage
/// task commits them together. Dropping the task releases its reservations.
pub(crate) struct Mutation {
    tree: Arc<Mutex<Tree>>,
    plan: Option<Plan>,
}

impl Mutation {
    pub(crate) fn commit(mut self) -> Result<()> {
        let plan = self.plan.take().expect("mutation consumed exactly once");
        let mut tree = lock(&self.tree);
        plan.release(&mut tree);
        tree.active()?;
        match plan {
            Plan::Remove { path, deleted } => {
                let removed = deleted.iter().filter_map(|key| match &tree.entries.get(key)?.data {
                    Data::File(file) => Some(file.bytes.len() as u64),
                    Data::Directory(_) => None,
                }).sum();
                tree.charge(removed, 0)?;
                if let Some((name, parent)) = path.split_last() {
                    tree.dir_mut(parent)?.remove(name);
                }
                for key in deleted {
                    if let Some(Entry { data: Data::File(file), .. }) = tree.entries.remove(&key) {
                        tree.used -= file.bytes.len() as u64;
                    }
                }
                if path.is_empty() {
                    tree.entries.insert(Path::new(), Entry::new(Kind::Directory));
                }
            }
            Plan::Move { source, target, locator } if source != target => {
                let (old_name, old_parent) = source.split_last().expect("files have a parent");
                let (new_name, new_parent) = target.split_last().expect("files have a parent");
                if let Some(Entry { data: Data::File(previous), .. }) = tree.entries.get(&target) {
                    tree.charge(previous.bytes.len() as u64, 0)?;
                }
                tree.dir_mut(old_parent)?.remove(old_name);
                tree.dir_mut(new_parent)?.insert(new_name.clone());
                let file = tree.entries.remove(&source).expect("reserved source exists");
                if let Some(Entry { data: Data::File(previous), .. }) = tree.entries.insert(target.clone(), file) {
                    tree.used -= previous.bytes.len() as u64;
                }
                *lock(&locator) = target;
            }
            Plan::Move { .. } => {}
        }
        Ok(())
    }
}

impl Drop for Mutation {
    fn drop(&mut self) {
        if let Some(plan) = self.plan.take() {
            plan.release(&mut lock(&self.tree));
        }
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
    reservation: Option<Reservation>,
    cursor: u64,
    closed: bool,
}

impl Lease {
    fn active(&self) -> Result<()> {
        if self.closed { Err(Error::Closed) } else { lock(&self.tree).active().map_err(|_| Error::Closed) }
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
        let committed = tree.file_mut(&self.path)?.bytes.len() as u64;
        let growth = requested.saturating_sub(committed);
        // Reserve shared quota before allocating. Staged bytes are invisible
        // to estimate(); a synchronous write commits in this same turn.
        let immediate = if self.pending.is_none() {
            tree.bucket.as_ref().map(|bucket| bucket.reserve(Endpoint::FileSystem, growth)).transpose()?
        } else {
            None
        };
        let mut old_reservation = 0;
        if let Some(r) = &mut self.reservation {
            old_reservation = r.bytes();
            if growth > old_reservation {
                r.resize(growth)?;
            }
        }
        if length > previous {
            let bytes = match &mut self.pending {
                Some(buffer) => buffer,
                None => &mut tree.file_mut(&self.path)?.bytes,
            };
            if bytes.try_reserve_exact(length - previous).is_err() {
                if let Some(r) = &mut self.reservation {
                    if growth > old_reservation { r.resize(old_reservation)?; }
                }
                return Err(Error::Quota);
            }
        }
        if let Some(r) = &mut self.reservation {
            if growth < old_reservation { r.resize(growth)?; }
        }
        if let Some(r) = immediate {
            r.commit(previous as u64, requested)?;
        }
        let bytes = match &mut self.pending {
            Some(buffer) => buffer,
            None => &mut tree.file_mut(&self.path)?.bytes,
        };
        if length > bytes.len() {
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
        // Release local resources on both retirement and quota failure.
        // Other siloed streams can have changed the committed size since the
        // last write; revalidate the replacement charge against that size.
        self.closed = true;
        let pending = self.pending.take();
        tree.staged -= pending.as_ref().map_or(0, |b| b.len() as u64);
        let mut reservation = self.reservation.take();
        if let Ok(file) = tree.file_mut(&self.path) {
            file.release();
        }
        if !commit {
            return Ok(());
        }
        tree.active().map_err(|_| Error::Closed)?;
        if let Some(buffer) = pending {
            let next = buffer.len() as u64;
            let old = tree.file_mut(&self.path)?.bytes.len() as u64;
            if let Some(mut reserved) = reservation.take() {
                reserved.resize(next.saturating_sub(old))?;
                reserved.commit(old, next)?;
            }
            let file = tree.file_mut(&self.path)?;
            file.bytes = buffer;
            file.mtime = now_ms();
            tree.used = tree.used - old + next;
        }
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

    use crate::storage_buckets::catalog::{Catalog, Options, StorageKey};

    fn storage_key() -> StorageKey {
        StorageKey {
            origin: "https://example.org".into(),
            top_level_site: Some("https://example.org".into()),
            cross_site_ancestor: false,
        }
    }

    #[test]
    fn named_bucket_removal_retires_old_handles_writers_and_pending_mutations() {
        let catalog = Catalog::default();
        let key = storage_key();
        let bucket = catalog.open(&key, "work", Options::default()).unwrap();
        let root = bucket.opfs.root(&bucket.origin);
        let default = Namespace::default().root(&key.origin);
        assert!(!root.is_same_entry(&default));
        let file = root.get("data", Kind::File, true).unwrap();
        let extra = root.get("extra", Kind::File, true).unwrap();
        let removal = extra.reserve_remove(false).unwrap();
        let mut stream = file.writable(false, false).unwrap();
        stream.write(b"staged", None).unwrap();
        assert_eq!(file.snapshot().unwrap().0.len(), 0);
        assert!(matches!(default.get("data", Kind::File, false), Err(Error::Missing)));

        catalog.delete(&key, "work").unwrap();
        let next = catalog.open(&key, "work", Options::default()).unwrap();
        let new_root = next.opfs.root(&next.origin);
        assert!(!root.is_same_entry(&new_root));
        assert!(matches!(root.get("x", Kind::Directory, true), Err(Error::Missing)));
        assert!(matches!(file.snapshot(), Err(Error::Missing)));
        assert_eq!(stream.close(), Err(Error::Closed));
        assert_eq!(removal.commit(), Err(Error::Missing));
        assert!(new_root.next(None).unwrap().is_none());

        next.set_expires(now_ms() as f64 - 1.0).unwrap();
        assert!(matches!(new_root.next(None), Err(Error::Missing)));
    }

    #[test]
    fn named_bucket_quota_accounts_for_staging_competing_writers_and_removal() {
        let catalog = Catalog::default();
        let bucket = catalog.open(&storage_key(), "files", Options { quota: Some(6), ..Options::default() }).unwrap();
        let root = bucket.opfs.root(&bucket.origin);
        let file = root.get("f", Kind::File, true).unwrap();
        let mut sync = file.sync(Mode::ReadWrite).unwrap();
        sync.write(b"12345", None).unwrap();
        assert_eq!(sync.write(b"xx", None), Err(Error::Quota));
        assert_eq!(bucket.estimate().unwrap().file_system, 5);
        sync.close().unwrap();
        let mut first = file.writable(true, false).unwrap();
        let mut second = file.writable(false, false).unwrap();
        first.write(b"6", Some(5)).unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, 5);
        assert_eq!(bucket.lease.charge(Endpoint::Caches, 0, 1), Err(BucketError::QuotaExceeded));
        second.close().unwrap();
        bucket.lease.charge(Endpoint::Caches, 0, 5).unwrap();
        assert_eq!(first.close(), Err(Error::Quota));
        assert!(file.snapshot().unwrap().0.is_empty());
        assert_eq!(bucket.estimate().unwrap().file_system, 0);

        let mut abandoned = file.writable(false, true).unwrap();
        abandoned.write(b"x", None).unwrap();
        abandoned.abort().unwrap();
        let mut writer = file.writable(false, false).unwrap();
        writer.write(b"y", None).unwrap();
        writer.close().unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, 6);
        root.reserve_remove_entry("f", false).unwrap().commit().unwrap();
        assert_eq!(bucket.estimate().unwrap().usage, 5);
    }

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
        file.reserve_remove(false).unwrap().commit().unwrap();
        assert!(matches!(locator.snapshot(), Err(Error::Missing)));
        let replacement = dir.get("doc", Kind::File, true).unwrap();
        assert!(replacement.is_same_entry(&locator));
        assert_ne!(replacement.unique_id().unwrap(), old_id);
        replacement.reserve_move(Some(&root), Some("moved")).unwrap().commit().unwrap();
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
        assert!(matches!(root.reserve_remove(true), Err(Error::Locked)));
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
    fn move_reserves_both_locators_until_atomic_commit() {
        let root = Namespace::default().root("origin");
        let from = root.get("before", Kind::File, true).unwrap();
        let alias = root.get("before", Kind::File, false).unwrap();
        let target = root.get("after", Kind::File, true).unwrap();
        let independent = root.get("unrelated", Kind::File, true).unwrap();
        let mut input = from.sync(Mode::ReadWrite).unwrap();
        input.write(b"original", None).unwrap();
        input.close().unwrap();

        let mutation = from.reserve_move(None, Some("after")).unwrap();
        assert_eq!(from.name(), "before");
        assert!(alias.is_same_entry(&from));
        assert_eq!(from.snapshot().unwrap().0, b"original");
        assert_eq!(target.snapshot().unwrap().0, b"");
        for blocked in [&from, &alias, &target] {
            assert!(matches!(blocked.sync(Mode::ReadOnly), Err(Error::Locked)));
            assert!(matches!(blocked.writable(false, false), Err(Error::Locked)));
            assert!(matches!(blocked.reserve_remove(false), Err(Error::Locked)));
        }
        independent.writable(false, true).unwrap().close().unwrap();
        assert!(matches!(root.reserve_remove(true), Err(Error::Locked)));

        mutation.commit().unwrap();
        assert_eq!(from.name(), "after");
        assert!(from.is_same_entry(&target));
        assert!(matches!(alias.snapshot(), Err(Error::Missing)));
        assert_eq!(target.snapshot().unwrap().0, b"original");
        target.writable(true, true).unwrap().close().unwrap();
        from.sync(Mode::ReadWrite).unwrap().close().unwrap();
    }

    #[test]
    fn cancellation_and_failed_acquisition_release_the_entire_reservation() {
        let root = Namespace::default().root("origin");
        let dir = root.get("tree", Kind::Directory, true).unwrap();
        let first = dir.get("first", Kind::File, true).unwrap();
        let last = dir.get("last", Kind::File, true).unwrap();

        let held = last.sync(Mode::ReadOnly).unwrap();
        assert!(matches!(first.reserve_move(None, Some("last")), Err(Error::Locked)));
        assert!(matches!(root.reserve_remove_entry("tree", true), Err(Error::Locked)));
        first.writable(false, true).unwrap().close().unwrap();
        drop(held);

        let staged = first.reserve_move(None, Some("new")).unwrap();
        assert!(matches!(dir.get("new", Kind::File, true), Err(Error::Locked)));
        assert!(matches!(dir.get("new", Kind::File, false), Err(Error::Missing)));
        drop(staged);
        assert_eq!(first.name(), "first");
        let new = dir.get("new", Kind::File, true).unwrap();
        let removal = root.reserve_remove_entry("tree", true).unwrap();
        assert_eq!(dir.next(None).unwrap().unwrap().0, "first");
        assert!(matches!(dir.get("another", Kind::File, true), Err(Error::Locked)));
        assert!(matches!(first.sync(Mode::ReadWrite), Err(Error::Locked)));
        assert!(matches!(last.writable(false, false), Err(Error::Locked)));
        assert!(matches!(new.reserve_move(Some(&root), None), Err(Error::Locked)));
        root.get("outside", Kind::File, true).unwrap().sync(Mode::ReadWrite).unwrap();
        drop(removal);
        first.sync(Mode::ReadWrite).unwrap().close().unwrap();
        root.reserve_remove_entry("tree", true).unwrap().commit().unwrap();
        assert!(matches!(new.snapshot(), Err(Error::Missing)));
        assert_eq!(root.next(None).unwrap().unwrap().0, "outside");
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
