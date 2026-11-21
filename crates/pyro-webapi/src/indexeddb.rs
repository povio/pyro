//! IndexedDB (W3C Indexed Database API 3.0), in memory.
//!
//! Data lives natively in the context's local storage partition, per origin
//! and database name (`Namespace`, shared with the context's documents and the
//! dedicated workers they start), so it survives navigation and is shared like
//! `localStorage`. Records are kept in key order: keys are encoded by idb.js
//! into ASCII strings whose byte order is IndexedDB key order, values are
//! stored in the worker wire format (structured clone). No disk persistence.
//!
//! The API objects, request/transaction state machine and key/key path rules
//! are idb.js, compiled on a realm's first IndexedDB use: realms that never
//! touch `indexedDB` or an `IDB*` name pay nothing beyond the accessors.
//! Natives here store data, keep the connection, lock and upgrade registry
//! that coordinates realms (other documents, worlds and worker threads), and
//! run idb.js's tasks: each IDB task is its own event-loop task whose events
//! are dispatched natively, so microtasks run after every listener while the
//! transaction is still active, as in browsers.
//!
//! Limits (explicit): Blob/File values are not cloneable (DataCloneError);
//! transactions are deactivated in the task after the one that created them,
//! not after that task's microtask checkpoint.
use crate::page::{PageDom, RemotePoster};
use crate::structured_clone::{decode_message, encode_message, WireMessage};
use crate::util::arg_string;
use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Persistent, RootedVec, Scope};
use pyro_js::{Value, WeakRealm};
use rustc_hash::FxHashMap;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    ops::Bound,
    rc::{Rc, Weak},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

/// Largest key generator value (2^53).
const MAX_GENERATED: f64 = 9007199254740992.0;
/// Sorts after every encoded key (they start with '1'..'5').
const KEY_MAX: &str = "~";

/// The key generator's number after `n`. 2^53 + 1 is not a float64, so the
/// generator is marked exhausted with infinity instead (`n + 1.0` would round
/// back to 2^53 and hand out that key again).
fn after(n: f64) -> f64 {
    if n >= MAX_GENERATED {
        f64::INFINITY
    } else {
        n + 1.0
    }
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);
fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// The context partition's databases, by origin. Clone to share.
#[derive(Clone, Default)]
pub(crate) struct Namespace(Arc<Mutex<Server>>);

#[derive(Default)]
struct Server {
    origins: FxHashMap<String, BTreeMap<String, Db>>,
    /// How to reach each realm that uses this namespace.
    realms: FxHashMap<u64, Remote>,
}

#[derive(Clone)]
struct Remote {
    thread: std::thread::ThreadId,
    poster: Option<RemotePoster>,
}

#[derive(Default)]
struct Db {
    /// 0 while the creating upgrade runs.
    version: u64,
    stores: BTreeMap<String, Store>,
    conns: Vec<Conn>,
    /// Realm running an upgrade or deletion; others wait.
    busy: Option<u64>,
    locks: Vec<Lock>,
    watchers: Vec<u64>,
    undo: FxHashMap<u64, Vec<Undo>>,
    upgrade: Option<Upgrade>,
}

struct Conn {
    id: u64,
    realm: u64,
}

struct Lock {
    tx: u64,
    realm: u64,
    stores: Vec<String>,
    write: bool,
}

struct Upgrade {
    tx: u64,
    old_version: u64,
    old_stores: BTreeMap<String, Store>,
}

#[derive(Clone)]
struct Store {
    key_path: String,
    auto_increment: bool,
    current: f64,
    records: BTreeMap<String, Record>,
    indexes: BTreeMap<String, Index>,
}

#[derive(Clone)]
struct Record {
    value: Arc<WireMessage>,
    /// Index id -> the keys this record has in it.
    keys: Vec<(u64, Vec<String>)>,
}

#[derive(Clone)]
struct Index {
    id: u64,
    key_path: String,
    unique: bool,
    multi_entry: bool,
    /// (index key, primary key).
    entries: BTreeSet<(String, String)>,
}

enum Undo {
    Record { store: String, key: String, old: Option<Record> },
    Generator { store: String, old: f64 },
    Store { name: String, old: Box<Store> },
}

#[derive(Clone)]
struct DatabaseInfo {
    name: String,
    version: u64,
}

impl Namespace {
    fn lock(&self) -> MutexGuard<'_, Server> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn databases(&self, origin: &str) -> Vec<DatabaseInfo> {
        let server = self.lock();
        let Some(dbs) = server.origins.get(origin) else { return Vec::new() };
        dbs.iter()
            .filter(|(_, db)| db.version > 0)
            .map(|(name, db)| DatabaseInfo { name: name.clone(), version: db.version })
            .collect()
    }

    /// Drops a realm (its document went away): closes its connections,
    /// rolls back its transactions and upgrades, and wakes waiting realms.
    fn release_realm(&self, handle: u64) {
        let mut wakes = Vec::new();
        {
            let mut server = self.lock();
            server.realms.remove(&handle);
            for dbs in server.origins.values_mut() {
                let mut gone = Vec::new();
                for (name, db) in dbs.iter_mut() {
                    let before = (db.conns.len(), db.locks.len(), db.busy);
                    db.conns.retain(|c| c.realm != handle);
                    db.watchers.retain(|&w| w != handle);
                    let txs: Vec<u64> = db.locks.iter().filter(|l| l.realm == handle).map(|l| l.tx).collect();
                    for tx in txs {
                        db.rollback(tx);
                    }
                    if db.busy == Some(handle) {
                        if let Some(upgrade) = db.upgrade.take() {
                            db.version = upgrade.old_version;
                            db.stores = upgrade.old_stores;
                        }
                        db.busy = None;
                    }
                    if before != (db.conns.len(), db.locks.len(), db.busy) {
                        wakes.extend(db.watchers.iter().map(|&w| (w, name.clone())));
                    }
                    if db.version == 0 && db.conns.is_empty() && db.busy.is_none() {
                        gone.push(name.clone());
                    }
                }
                for name in gone {
                    dbs.remove(&name);
                }
            }
            let wakes: Vec<_> =
                wakes.into_iter().filter_map(|(w, n)| Some((server.realms.get(&w)?.clone(), w, n))).collect();
            drop(server);
            for (remote, realm, name) in wakes {
                deliver(&remote, realm, Message::Wake(name));
            }
        }
    }
}

impl Db {
    fn store(&mut self, name: &str) -> JsResult<&mut Store> {
        self.stores.get_mut(name).ok_or_else(|| JsError::dom("InvalidStateError", "The object store was deleted."))
    }

    /// Records an undo step unless `tx` is the upgrade (which snapshots).
    fn log(&mut self, tx: u64, undo: impl FnOnce() -> Undo) {
        if self.upgrade.as_ref().is_some_and(|u| u.tx == tx) {
            return;
        }
        self.undo.entry(tx).or_default().push(undo());
    }

    fn rollback(&mut self, tx: u64) {
        self.locks.retain(|l| l.tx != tx);
        let Some(steps) = self.undo.remove(&tx) else { return };
        for step in steps.into_iter().rev() {
            match step {
                Undo::Record { store, key, old } => {
                    if let Some(s) = self.stores.get_mut(&store) {
                        s.remove(&key);
                        if let Some(old) = old {
                            s.insert(key, old);
                        }
                    }
                }
                Undo::Generator { store, old } => {
                    if let Some(s) = self.stores.get_mut(&store) {
                        s.current = old;
                    }
                }
                Undo::Store { name, old } => {
                    self.stores.insert(name, *old);
                }
            }
        }
    }
}

impl Store {
    fn remove(&mut self, key: &str) -> Option<Record> {
        let record = self.records.remove(key)?;
        for (id, keys) in &record.keys {
            if let Some(index) = self.indexes.values_mut().find(|i| i.id == *id) {
                for k in keys {
                    index.entries.remove(&(k.clone(), key.to_owned()));
                }
            }
        }
        Some(record)
    }

    fn insert(&mut self, key: String, record: Record) {
        for (id, keys) in &record.keys {
            if let Some(index) = self.indexes.values_mut().find(|i| i.id == *id) {
                for k in keys {
                    index.entries.insert((k.clone(), key.clone()));
                }
            }
        }
        self.records.insert(key, record);
    }
}

/// A key range of encoded keys.
struct Range {
    lower: Option<String>,
    upper: Option<String>,
    lower_open: bool,
    upper_open: bool,
}

impl Range {
    fn lower_bound(&self) -> Bound<(String, String)> {
        match &self.lower {
            None => Bound::Unbounded,
            Some(l) if self.lower_open => Bound::Excluded((l.clone(), KEY_MAX.into())),
            Some(l) => Bound::Included((l.clone(), String::new())),
        }
    }
    fn upper_bound(&self) -> Bound<(String, String)> {
        match &self.upper {
            None => Bound::Unbounded,
            Some(u) if self.upper_open => Bound::Excluded((u.clone(), String::new())),
            Some(u) => Bound::Included((u.clone(), KEY_MAX.into())),
        }
    }
    fn contains(&self, key: &str) -> bool {
        let above = match &self.lower {
            None => true,
            Some(l) => key > l.as_str() || (!self.lower_open && key == l),
        };
        let below = match &self.upper {
            None => true,
            Some(u) => key < u.as_str() || (!self.upper_open && key == u),
        };
        above && below
    }
}

/// The stricter of two lower (`upper` false) or upper bounds.
fn tighter(a: Bound<(String, String)>, b: Bound<(String, String)>, upper: bool) -> Bound<(String, String)> {
    use Bound::*;
    let value = |b: &Bound<(String, String)>| match b {
        Included(v) | Excluded(v) => Some(v.clone()),
        Unbounded => None,
    };
    match (value(&a), value(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some(x), Some(y)) => {
            let a_wins = if upper { x < y } else { x > y };
            if x == y {
                if matches!(a, Excluded(_)) {
                    a
                } else {
                    b
                }
            } else if a_wins {
                a
            } else {
                b
            }
        }
    }
}

/// An ordered set of (key, primary key) pairs: an index, or an object store
/// whose pairs are (key, key).
enum Source<'a> {
    Store(&'a BTreeMap<String, Record>),
    Index(&'a BTreeSet<(String, String)>),
}

impl Source<'_> {
    /// For a store, a pair bound as a key bound ("" sorts before and "~" after
    /// every key, so the pair compares as its key against (x, x)).
    fn key_bound(b: &Bound<(String, String)>, upper: bool) -> Bound<String> {
        match b {
            Bound::Unbounded => Bound::Unbounded,
            Bound::Included((k, p)) => {
                if (!upper && p <= k) || (upper && k <= p) {
                    Bound::Included(k.clone())
                } else {
                    Bound::Excluded(k.clone())
                }
            }
            Bound::Excluded((k, p)) => {
                if (!upper && p < k) || (upper && k < p) {
                    Bound::Included(k.clone())
                } else {
                    Bound::Excluded(k.clone())
                }
            }
        }
    }

    /// First pair at or after `lower`.
    fn first(&self, lower: &Bound<(String, String)>) -> Option<(String, String)> {
        match self {
            Source::Index(set) => set.range((lower.clone(), Bound::Unbounded)).next().cloned(),
            Source::Store(map) => {
                let b = Self::key_bound(lower, false);
                map.range::<String, _>((b, Bound::Unbounded)).next().map(|(k, _)| (k.clone(), k.clone()))
            }
        }
    }

    /// Last pair at or before `upper`.
    fn last(&self, upper: &Bound<(String, String)>) -> Option<(String, String)> {
        match self {
            Source::Index(set) => set.range((Bound::Unbounded, upper.clone())).next_back().cloned(),
            Source::Store(map) => {
                let b = Self::key_bound(upper, true);
                map.range::<String, _>((Bound::Unbounded, b)).next_back().map(|(k, _)| (k.clone(), k.clone()))
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Direction {
    Next,
    NextUnique,
    Prev,
    PrevUnique,
}

impl Direction {
    fn from(n: f64) -> Direction {
        match n as u32 {
            1 => Direction::NextUnique,
            2 => Direction::Prev,
            3 => Direction::PrevUnique,
            _ => Direction::Next,
        }
    }
    fn forward(self) -> bool {
        matches!(self, Direction::Next | Direction::NextUnique)
    }
}

/// IndexedDB "iterate a cursor": the `count`th pair after `position` in
/// `dir` within `range`, at or past `target` (continue(key) /
/// continuePrimaryKey(key, primaryKey)).
fn seek(
    src: &Source<'_>,
    range: &Range,
    dir: Direction,
    mut position: Option<(String, String)>,
    target: Option<(String, Option<String>)>,
    count: u32,
) -> Option<(String, String)> {
    let mut target = target;
    let mut found = None;
    for _ in 0..count.max(1) {
        let next = if dir.forward() {
            let mut bound = range.lower_bound();
            if let Some((k, p)) = &position {
                let after = match dir {
                    Direction::NextUnique => Bound::Excluded((k.clone(), KEY_MAX.into())),
                    _ => Bound::Excluded((k.clone(), p.clone())),
                };
                bound = tighter(bound, after, false);
            }
            if let Some((k, p)) = target.take() {
                bound = tighter(bound, Bound::Included((k, p.unwrap_or_default())), false);
            }
            src.first(&bound).filter(|(k, _)| range.contains(k))
        } else {
            let mut bound = range.upper_bound();
            if let Some((k, p)) = &position {
                let before = match dir {
                    Direction::PrevUnique => Bound::Excluded((k.clone(), String::new())),
                    _ => Bound::Excluded((k.clone(), p.clone())),
                };
                bound = tighter(bound, before, true);
            }
            if let Some((k, p)) = target.take() {
                bound = tighter(bound, Bound::Included((k, p.unwrap_or_else(|| KEY_MAX.into()))), true);
            }
            let last = src.last(&bound).filter(|(k, _)| range.contains(k));
            // prevunique yields each key's first record.
            match (last, dir) {
                (Some((k, _)), Direction::PrevUnique) => src.first(&Bound::Included((k, String::new()))),
                (last, _) => last,
            }
        };
        match next {
            Some(pair) => {
                position = Some(pair.clone());
                found = Some(pair);
            }
            None => return None,
        }
    }
    found
}

/// A message to another realm, delivered in a task there.
enum Message {
    /// Something `name`'s waiters wait for may have changed.
    Wake(String),
    /// `versionchange` at connection `conn` (old, new; new is None for deletion).
    VersionChange { conn: u64, old: u64, new: Option<u64> },
}

/// One realm's IndexedDB module (this thread's realms).
struct Entry {
    realm: WeakRealm,
    page: Weak<PageDom>,
    module: Persistent,
    namespace: Namespace,
}

thread_local! {
    static REALMS: RefCell<FxHashMap<u64, Entry>> = RefCell::default();
}

fn deliver(remote: &Remote, realm: u64, message: Message) {
    if remote.thread == std::thread::current().id() {
        let page = REALMS.with(|r| r.borrow().get(&realm).and_then(|e| e.page.upgrade()));
        if let Some(page) = page {
            page.post_task(Box::new(move || receive(realm, message)));
        }
    } else if let Some(poster) = &remote.poster {
        poster(Box::new(move || receive(realm, message)));
    }
}

/// Runs on the realm's thread, in a task.
fn receive(handle: u64, message: Message) {
    let Some((realm, module)) = REALMS.with(|r| r.borrow().get(&handle).map(|e| (e.realm.clone(), e.module.clone())))
    else {
        return;
    };
    let Some(realm) = realm.upgrade() else { return };
    let Ok(s) = realm.scope() else { return };
    let result = (|| -> JsResult<()> {
        let module = module.get(&s).as_object().ok_or_else(unavailable)?;
        let wake = module.get_atom("wake")?.as_object().ok_or_else(unavailable)?;
        match message {
            Message::Wake(name) => wake.call(None, &[s.number(0.0), s.string(&name)])?,
            Message::VersionChange { conn, old, new } => wake.call(
                None,
                &[
                    s.number(1.0),
                    s.undefined(),
                    s.number(conn as f64),
                    s.number(old as f64),
                    new.map_or(s.null(), |n| s.number(n as f64)),
                ],
            )?,
        };
        Ok(())
    })();
    if let Err(error) = result {
        s.report_exception(&error);
    }
}

/// Runs one idb.js task: its operation, then natively the event it returns.
fn run_step(handle: u64) {
    let Some((realm, module, page)) =
        REALMS.with(|r| r.borrow().get(&handle).map(|e| (e.realm.clone(), e.module.clone(), e.page.clone())))
    else {
        return;
    };
    let (Some(realm), Some(page)) = (realm.upgrade(), page.upgrade()) else { return };
    if page.is_disposed() {
        return;
    }
    let Ok(s) = realm.scope() else { return };
    let result = (|| -> JsResult<()> {
        let module = module.get(&s).as_object().ok_or_else(unavailable)?;
        let step = module.get_atom("step")?.as_object().ok_or_else(unavailable)?;
        let Some(job) = step.call(None, &[])?.as_object() else { return Ok(()) };
        let target = job.get_atom("t")?;
        let event = job.get_atom("e")?;
        let after = job.get_atom("a")?;
        let key = s.unwrap::<crate::target::EventTarget>(target).map(|t| t.key);
        let event = s.unwrap::<crate::event::Event>(event).map(|e| (*e).clone());
        let (Some(key), Some(event)) = (key, event) else { return Err(unavailable()) };
        event.state.borrow_mut().is_trusted = true;
        // Native dispatch: each listener is an outermost call, so microtasks
        // run after it while the transaction is still active.
        let (not_canceled, threw) = crate::target::fire_noting_exceptions(&page, key, &event);
        if let Some(after) = after.as_object() {
            after.call(None, &[s.boolean(not_canceled), s.boolean(threw)])?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        s.report_exception(&error);
    }
}

/// Document disposal: the page's realms close their connections and abort
/// their transactions. Nothing to do for pages that never used IndexedDB.
pub(crate) fn dispose(page: &PageDom) {
    let gone: Vec<(u64, Entry)> = REALMS.with(|r| {
        let mut realms = r.borrow_mut();
        if realms.is_empty() {
            return Vec::new();
        }
        let handles: Vec<u64> =
            realms.iter().filter(|(_, e)| std::ptr::eq(e.page.as_ptr(), page)).map(|(h, _)| *h).collect();
        handles.into_iter().filter_map(|h| realms.remove(&h).map(|e| (h, e))).collect()
    });
    for (handle, entry) in gone {
        entry.namespace.release_realm(handle);
        drop(entry);
    }
}

fn unavailable() -> JsError {
    JsError::Type("IndexedDB is unavailable".into())
}

fn unknown(what: &str) -> JsError {
    JsError::dom("UnknownError", format!("The database {what} no longer exists."))
}

pub struct FactoryData {
    page: Weak<PageDom>,
    origin: Option<String>,
    namespace: Namespace,
}
impl FactoryData {
    fn origin(&self) -> JsResult<&str> {
        if self.page.upgrade().is_none_or(|page| page.is_disposed()) {
            return Err(JsError::ContextDestroyed);
        }
        self.origin
            .as_deref()
            .ok_or_else(|| JsError::dom("SecurityError", "IndexedDB is unavailable for an opaque origin."))
    }
}

const MODULE: &str = "lpIdb";

/// The realm's idb.js module, made on first use (kept on the factory wrapper).
fn factory_module<'s>(s: &Scope<'s>, factory: Object<'s>, data: &FactoryData) -> JsResult<Object<'s>> {
    if let Some(module) = s.get_hidden(factory, MODULE)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let origin = data.origin()?.to_owned();
    let page = data.page.upgrade().ok_or(JsError::ContextDestroyed)?;
    let handle = next_id();
    let natives = natives(s, &page, data.namespace.clone(), origin, handle)?;
    let root = Persistent::new(s, natives.value());
    let make = s.eval(include_str!("idb.js"), Some("pyro://internal/idb.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[root.get(s)])?;
    let object = module.as_object().ok_or_else(unavailable)?;
    s.set_hidden(factory, MODULE, module)?;
    data.namespace
        .lock()
        .realms
        .insert(handle, Remote { thread: std::thread::current().id(), poster: page.remote_poster() });
    REALMS.with(|r| {
        r.borrow_mut().insert(
            handle,
            Entry {
                realm: s.realm().downgrade(),
                page: Rc::downgrade(&page),
                module: Persistent::new(s, module),
                namespace: data.namespace.clone(),
            },
        )
    });
    Ok(object)
}

fn call_module<'s>(_s: &Scope<'s>, module: Object<'s>, name: &str, args: &[Value<'s>]) -> JsResult<Value<'s>> {
    module.get(name)?.as_object().ok_or_else(unavailable)?.call(None, args)
}

pub struct IdbFactory;
impl JsClass for IdbFactory {
    const NAME: &'static str = "IDBFactory";
    type Data = FactoryData;
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.promise_method("databases", 0, |s, data, _| {
            let promise = s.deferred()?;
            match data.origin() {
                Ok(origin) => {
                    let mut rows = RootedVec::new();
                    // Snapshot native metadata before invoking JS setters or
                    // resolving promises; no namespace borrow crosses JS entry.
                    for info in data.namespace.databases(origin) {
                        let row = s.object();
                        row.set_atom("name", s.string(&info.name))?;
                        row.set_atom("version", s.number(info.version as f64))?;
                        rows.push(s, row.value());
                    }
                    promise.resolve(s, s.array(&rows.iter(s).collect::<Vec<_>>())?.value())?;
                }
                Err(error) => promise.reject(s, error.to_value(s))?,
            }
            Ok(promise.promise.get(s))
        });
        def.method_this("open", 1, |s, data, this, args| {
            args.require(1, "IDBFactory.open")?;
            data.origin()?;
            let module = factory_module(s, this, data)?;
            call_module(s, module, "open", &[args.get(0), args.get(1), s.number(args.len() as f64)])
        });
        def.method_this("deleteDatabase", 1, |s, data, this, args| {
            let name = arg_string(&args, 0, "IDBFactory.deleteDatabase")?;
            data.origin()?;
            let module = factory_module(s, this, data)?;
            call_module(s, module, "deleteDatabase", &[s.string(&name)])
        });
        def.method_this("cmp", 2, |s, data, this, args| {
            args.require(2, "IDBFactory.cmp")?;
            // Comparing keys needs no storage: opaque origins may too.
            let module = match data.origin() {
                Ok(_) => factory_module(s, this, data)?,
                Err(JsError::ContextDestroyed) => return Err(JsError::ContextDestroyed),
                Err(_) => opaque_module(s, this)?,
            };
            call_module(s, module, "cmp", &[args.get(0), args.get(1)])
        });
    }
}

pub(crate) fn window_factory<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    if page.is_disposed() {
        return Err(JsError::ContextDestroyed);
    }
    let key = IdentityKey(page.id() | (1 << 57), 2);
    crate::window::same_object(s, page, key, || {
        let data = FactoryData {
            page: Rc::downgrade(page),
            origin: crate::storage::storage_origin(s, page).ok(),
            namespace: page.local_storage.borrow().indexed_db(),
        };
        s.wrap::<IdbFactory>(data, Some(key))
    })
}

/// A global's page (Window, worker scope).
pub(crate) trait PageOf {
    fn page_of(&self) -> &Rc<PageDom>;
}
impl<T: crate::window::WindowHost> PageOf for T {
    fn page_of(&self) -> &Rc<PageDom> {
        self.page()
    }
}

macro_rules! lazy_globals {
    ($def:ident, $($name:literal),+ $(,)?) => {
        $($def.global_value_accessor(
            $name,
            |s, g| global(s, PageOf::page_of(g), $name),
            |s, g, value| replace(s, PageOf::page_of(g), $name, value),
        );)+
    };
}

/// Adds the lazy names of the interfaces idb.js defines to a global class.
pub(crate) fn define_globals<C: JsClass>(def: &mut ClassDef<C>)
where
    C::Data: PageOf,
{
    lazy_globals!(
        def,
        "IDBRequest",
        "IDBOpenDBRequest",
        "IDBDatabase",
        "IDBTransaction",
        "IDBObjectStore",
        "IDBIndex",
        "IDBCursor",
        "IDBCursorWithValue",
        "IDBKeyRange",
        "IDBVersionChangeEvent",
        "IDBRecord",
        "DOMStringList",
    );
}

fn global<'s>(s: &Scope<'s>, page: &Rc<PageDom>, name: &str) -> JsResult<Value<'s>> {
    let factory = window_factory(s, page)?.as_object().ok_or_else(unavailable)?;
    let data = s.unwrap::<IdbFactory>(factory.value()).ok_or_else(unavailable)?;
    // Opaque origins still get the interfaces (feature detection), through a
    // module whose operations reject before touching storage.
    let module = match data.origin() {
        Ok(_) => factory_module(s, factory, &data)?,
        Err(JsError::ContextDestroyed) => return Err(JsError::ContextDestroyed),
        Err(_) => opaque_module(s, factory)?,
    };
    let value = module.get_atom("globals")?.as_object().ok_or_else(unavailable)?.get(name)?;
    replace(s, page, name, value)?;
    Ok(value)
}

/// Per-object host state for IDB event-handler accessors. Keeping it on the
/// EventTarget lets a borrowed same-origin accessor reach the owner's state.
fn binding_natives<'s>(s: &Scope<'s>) -> JsResult<Object<'s>> {
    let natives = s.object();
    let state = s.function("handlerState", |s, _, args| {
        const KEY: &str = "lpIDBEventHandlers";
        let Some(object) = args.get(0).as_object() else { return Ok(s.undefined()) };
        if !args.get(1).is_undefined() {
            s.set_hidden(object, KEY, args.get(1))?;
            return Ok(s.undefined());
        }
        Ok(s.get_hidden(object, KEY)?.unwrap_or(s.undefined()))
    })?;
    natives.set_atom("handlerState", state.value())?;
    let set = s.function("setHandler", |s, _, args| {
        let target = s.unwrap::<crate::target::EventTarget>(args.get(0)).ok_or_else(unavailable)?;
        let name = args.get(1).to_string()?;
        let ty = ["success", "error", "blocked", "upgradeneeded", "abort", "close", "versionchange", "complete"]
            .into_iter().find(|&value| value == name).ok_or_else(unavailable)?;
        crate::target::set_event_handler(s, &target, ty, args.get(2))?;
        Ok(s.undefined())
    })?;
    natives.set_atom("setHandler", set.value())?;
    let cancel = s.function("cancelEvent", |s, _, args| {
        let event = s.unwrap::<crate::event::Event>(args.get(0)).ok_or_else(unavailable)?;
        event.state.borrow_mut().prevent_default();
        Ok(s.undefined())
    })?;
    natives.set_atom("cancelEvent", cancel.value())?;
    Ok(natives)
}

/// Interfaces only, for a realm with an opaque origin (no storage, no tasks).
fn opaque_module<'s>(s: &Scope<'s>, factory: Object<'s>) -> JsResult<Object<'s>> {
    if let Some(module) = s.get_hidden(factory, MODULE)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let natives = binding_natives(s)?;
    let make = s.eval(include_str!("idb.js"), Some("pyro://internal/idb.js"), 1)?;
    let module = make.as_object().ok_or_else(unavailable)?.call(None, &[natives.value()])?;
    s.set_hidden(factory, MODULE, module)?;
    module.as_object().ok_or_else(unavailable)
}

fn replace(s: &Scope<'_>, page: &Rc<PageDom>, name: &str, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    // Worker realms keep no captured `Object.defineProperty`.
    let define = match define {
        Some(define) => define,
        None => s.global().get_atom("Object")?.as_object().ok_or_else(unavailable)?.get_atom("defineProperty")?,
    };
    let define = define.as_object().ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string(name), desc.value()])?;
    Ok(())
}

/// What a native closure needs: the partition, origin and realm handle.
struct Ctx {
    namespace: Namespace,
    origin: String,
    handle: u64,
    page: Weak<PageDom>,
}

impl Ctx {
    /// Runs `f` on database `name` of this origin (UnknownError when it is gone).
    fn db<R>(&self, name: &str, f: impl FnOnce(&mut Db) -> JsResult<R>) -> JsResult<R> {
        let mut server = self.namespace.lock();
        let db = server.origins.get_mut(&self.origin).and_then(|dbs| dbs.get_mut(name)).ok_or_else(|| unknown(name))?;
        f(db)
    }

    /// Wakes the other realms watching `name`.
    fn wake(&self, name: &str) {
        let targets: Vec<(Remote, u64)> = {
            let server = self.namespace.lock();
            let Some(db) = server.origins.get(&self.origin).and_then(|d| d.get(name)) else { return };
            db.watchers
                .iter()
                .filter(|&&w| w != self.handle)
                .filter_map(|w| Some((server.realms.get(w)?.clone(), *w)))
                .collect()
        };
        for (remote, realm) in targets {
            deliver(&remote, realm, Message::Wake(name.to_owned()));
        }
    }
}

fn opt_string(v: Value<'_>) -> JsResult<Option<String>> {
    if v.is_nullish() {
        Ok(None)
    } else {
        Ok(Some(v.to_string()?))
    }
}

fn range_at(a: &Args<'_>, i: usize) -> JsResult<Range> {
    Ok(Range {
        lower: opt_string(a.get(i))?,
        upper: opt_string(a.get(i + 1))?,
        lower_open: a.get(i + 2).to_bool(),
        upper_open: a.get(i + 3).to_bool(),
    })
}

fn strings(v: Value<'_>) -> JsResult<Vec<String>> {
    let Some(list) = v.as_object() else { return Ok(Vec::new()) };
    let n = list.get_atom("length")?.to_number()? as u32;
    (0..n).map(|i| list.get_index(i)?.to_string()).collect()
}

fn add<F>(s: &Scope<'_>, natives: &Object<'_>, ctx: &Rc<Ctx>, name: &'static str, f: F) -> JsResult<()>
where
    F: for<'a> Fn(&Scope<'a>, &Ctx, Args<'a>) -> JsResult<Value<'a>> + 'static,
{
    let ctx = ctx.clone();
    let function = s.function(name, move |s, _this, a| f(s, &ctx, a))?;
    natives.set(name, function.value())
}

fn natives<'s>(
    s: &Scope<'s>,
    page: &Rc<PageDom>,
    namespace: Namespace,
    origin: String,
    handle: u64,
) -> JsResult<Object<'s>> {
    let ctx = Rc::new(Ctx { namespace, origin, handle, page: Rc::downgrade(page) });
    let natives = binding_natives(s)?;
    // One task of idb.js's queue.
    add(s, &natives, &ctx, "post", |s, ctx, _a| {
        if let Some(page) = ctx.page.upgrade() {
            let handle = ctx.handle;
            page.post_task(Box::new(move || run_step(handle)));
        }
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "nextId", |s, _ctx, _a| Ok(s.number(next_id() as f64)))?;
    add(s, &natives, &ctx, "clone", |s, _ctx, a| crate::structured_clone::clone_for_storage(s, a.get(0)))?;
    // DOM "get the parent" of a request (its transaction) or transaction (its connection).
    add(s, &natives, &ctx, "setParent", |s, ctx, a| {
        let child = s.unwrap::<crate::target::EventTarget>(a.get(0)).map(|t| t.key);
        let parent = s.unwrap::<crate::target::EventTarget>(a.get(1)).map(|t| t.key);
        if let (Some(child), Some(page)) = (child, ctx.page.upgrade()) {
            page.dom.borrow_mut().set_target_parent(child, parent);
        }
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "watch", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let watch = a.get(1).to_bool();
        let mut server = ctx.namespace.lock();
        let dbs = server.origins.entry(ctx.origin.clone()).or_default();
        if let Some(db) = dbs.get_mut(&name) {
            db.watchers.retain(|&w| w != ctx.handle);
            if watch {
                db.watchers.push(ctx.handle);
            }
        }
        Ok(s.undefined())
    })?;
    // Takes the database for an upgrade or deletion; false while another realm has it.
    add(s, &natives, &ctx, "reserve", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let mut server = ctx.namespace.lock();
        let db = server.origins.entry(ctx.origin.clone()).or_default().entry(name).or_default();
        if db.busy.is_some_and(|b| b != ctx.handle) {
            return Ok(s.boolean(false));
        }
        db.busy = Some(ctx.handle);
        Ok(s.boolean(true))
    })?;
    // Whether another realm runs an upgrade or deletion.
    add(s, &natives, &ctx, "busy", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let server = ctx.namespace.lock();
        let busy = server.origins.get(&ctx.origin).and_then(|d| d.get(&name)).and_then(|db| db.busy);
        Ok(s.boolean(busy.is_some_and(|b| b != ctx.handle)))
    })?;
    add(s, &natives, &ctx, "release", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        {
            let mut server = ctx.namespace.lock();
            if let Some(dbs) = server.origins.get_mut(&ctx.origin) {
                let remove = dbs.get_mut(&name).is_some_and(|db| {
                    if db.busy == Some(ctx.handle) {
                        db.busy = None;
                    }
                    db.version == 0 && db.conns.is_empty() && db.watchers.is_empty() && db.busy.is_none()
                });
                if remove {
                    dbs.remove(&name);
                }
            }
        }
        ctx.wake(&name);
        Ok(s.undefined())
    })?;
    // The committed version, or -1 when there is no such database.
    add(s, &natives, &ctx, "version", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let server = ctx.namespace.lock();
        let db = server.origins.get(&ctx.origin).and_then(|d| d.get(&name));
        Ok(s.number(db.filter(|db| db.version > 0).map_or(-1.0, |db| db.version as f64)))
    })?;
    // [[store, keyPathJSON, autoIncrement, [[index, keyPathJSON, unique, multiEntry]...]]...]
    add(s, &natives, &ctx, "schema", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let rows: Vec<(String, String, bool, Vec<(String, String, bool, bool)>)> = ctx.db(&name, |db| {
            Ok(db
                .stores
                .iter()
                .map(|(n, st)| {
                    let ix = st
                        .indexes
                        .iter()
                        .map(|(i, x)| (i.clone(), x.key_path.clone(), x.unique, x.multi_entry))
                        .collect();
                    (n.clone(), st.key_path.clone(), st.auto_increment, ix)
                })
                .collect())
        })?;
        let mut stores = Vec::new();
        for (n, kp, ai, ix) in rows {
            let mut indexes = Vec::new();
            for (i, kp, u, m) in ix {
                indexes.push(s.array(&[s.string(&i), s.string(&kp), s.boolean(u), s.boolean(m)])?.value());
            }
            let indexes = s.array(&indexes)?;
            stores.push(s.array(&[s.string(&n), s.string(&kp), s.boolean(ai), indexes.value()])?.value());
        }
        Ok(s.array(&stores)?.value())
    })?;
    add(s, &natives, &ctx, "connect", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let id = next_id();
        let realm = ctx.handle;
        ctx.db(&name, |db| {
            db.conns.push(Conn { id, realm });
            Ok(())
        })?;
        Ok(s.number(id as f64))
    })?;
    add(s, &natives, &ctx, "disconnect", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let id = a.get(1).to_number()? as u64;
        {
            let mut server = ctx.namespace.lock();
            if let Some(dbs) = server.origins.get_mut(&ctx.origin) {
                let remove = dbs.get_mut(&name).is_some_and(|db| {
                    db.conns.retain(|c| c.id != id);
                    db.version == 0 && db.conns.is_empty() && db.busy.is_none() && db.watchers.is_empty()
                });
                if remove {
                    dbs.remove(&name);
                }
            }
        }
        ctx.wake(&name);
        Ok(s.undefined())
    })?;
    // Open connections of other realms (this realm counts its own).
    add(s, &natives, &ctx, "others", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let server = ctx.namespace.lock();
        let db = server.origins.get(&ctx.origin).and_then(|d| d.get(&name));
        let n = db.map_or(0, |db| db.conns.iter().filter(|c| c.realm != ctx.handle).count());
        Ok(s.number(n as f64))
    })?;
    // `versionchange` at other realms' connections.
    add(s, &natives, &ctx, "notify", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let old = a.get(1).to_number()? as u64;
        let new = if a.get(2).is_null() { None } else { Some(a.get(2).to_number()? as u64) };
        let targets: Vec<(Remote, u64, u64)> = {
            let server = ctx.namespace.lock();
            let db = server.origins.get(&ctx.origin).and_then(|d| d.get(&name));
            db.map(|db| {
                db.conns
                    .iter()
                    .filter(|c| c.realm != ctx.handle)
                    .filter_map(|c| Some((server.realms.get(&c.realm)?.clone(), c.realm, c.id)))
                    .collect()
            })
            .unwrap_or_default()
        };
        for (remote, realm, conn) in targets {
            deliver(&remote, realm, Message::VersionChange { conn, old, new });
        }
        Ok(s.undefined())
    })?;
    // A transaction may start unless another realm's running one conflicts.
    add(s, &natives, &ctx, "lock", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let tx = a.get(1).to_number()? as u64;
        let stores = strings(a.get(2))?;
        let write = a.get(3).to_bool();
        let realm = ctx.handle;
        let granted = ctx.db(&name, |db| {
            let conflict = db
                .locks
                .iter()
                .any(|l| l.realm != realm && (l.write || write) && l.stores.iter().any(|n| stores.contains(n)));
            if !conflict {
                db.locks.push(Lock { tx, realm, stores, write });
            }
            Ok(!conflict)
        })?;
        Ok(s.boolean(granted))
    })?;
    add(s, &natives, &ctx, "beginUpgrade", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let tx = a.get(1).to_number()? as u64;
        let version = a.get(2).to_number()? as u64;
        ctx.db(&name, |db| {
            db.upgrade = Some(Upgrade { tx, old_version: db.version, old_stores: db.stores.clone() });
            db.version = version;
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "commit", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let tx = a.get(1).to_number()? as u64;
        ctx.db(&name, |db| {
            db.undo.remove(&tx);
            db.locks.retain(|l| l.tx != tx);
            if db.upgrade.as_ref().is_some_and(|u| u.tx == tx) {
                db.upgrade = None;
            }
            Ok(())
        })?;
        ctx.wake(&name);
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "abort", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let tx = a.get(1).to_number()? as u64;
        ctx.db(&name, |db| {
            if db.upgrade.as_ref().is_some_and(|u| u.tx == tx) {
                let upgrade = db.upgrade.take().expect("upgrade");
                db.version = upgrade.old_version;
                db.stores = upgrade.old_stores;
            }
            db.rollback(tx);
            Ok(())
        })?;
        ctx.wake(&name);
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "deleteDb", |s, ctx, a| {
        let name = a.get(0).to_string()?;
        let mut server = ctx.namespace.lock();
        if let Some(db) = server.origins.get_mut(&ctx.origin).and_then(|d| d.get_mut(&name)) {
            db.version = 0;
            db.stores.clear();
            db.undo.clear();
            db.locks.clear();
            db.upgrade = None;
        }
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "createStore", |s, ctx, a| {
        let (name, store, key_path) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?);
        let auto_increment = a.get(3).to_bool();
        ctx.db(&name, |db| {
            db.stores.insert(
                store,
                Store { key_path, auto_increment, current: 1.0, records: BTreeMap::new(), indexes: BTreeMap::new() },
            );
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "deleteStore", |s, ctx, a| {
        let (name, store) = (a.get(0).to_string()?, a.get(1).to_string()?);
        ctx.db(&name, |db| {
            db.stores.remove(&store);
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "renameStore", |s, ctx, a| {
        let (name, old, new) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?);
        ctx.db(&name, |db| {
            if let Some(store) = db.stores.remove(&old) {
                db.stores.insert(new, store);
            }
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "createIndex", |s, ctx, a| {
        let (name, store, index, key_path) =
            (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?, a.get(3).to_string()?);
        let (unique, multi_entry) = (a.get(4).to_bool(), a.get(5).to_bool());
        ctx.db(&name, |db| {
            let id = next_id();
            db.store(&store)?
                .indexes
                .insert(index, Index { id, key_path, unique, multi_entry, entries: BTreeSet::new() });
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "deleteIndex", |s, ctx, a| {
        let (name, store, index) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?);
        ctx.db(&name, |db| {
            let st = db.store(&store)?;
            if let Some(ix) = st.indexes.remove(&index) {
                for record in st.records.values_mut() {
                    record.keys.retain(|(id, _)| *id != ix.id);
                }
            }
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "renameIndex", |s, ctx, a| {
        let (name, store, old, new) =
            (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?, a.get(3).to_string()?);
        ctx.db(&name, |db| {
            let st = db.store(&store)?;
            if let Some(ix) = st.indexes.remove(&old) {
                st.indexes.insert(new, ix);
            }
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    // Every record as [primaryKey, value, ...] (index population).
    add(s, &natives, &ctx, "records", |s, ctx, a| {
        let (name, store) = (a.get(0).to_string()?, a.get(1).to_string()?);
        let rows: Vec<(String, Arc<WireMessage>)> = ctx.db(&name, |db| {
            Ok(db.store(&store)?.records.iter().map(|(k, r)| (k.clone(), r.value.clone())).collect())
        })?;
        let mut out = RootedVec::new();
        for (key, value) in rows {
            out.push(s, s.string(&key));
            out.push(s, decode_message(s, &value)?);
        }
        Ok(s.array(&out.iter(s).collect::<Vec<_>>())?.value())
    })?;
    // Adds a record's keys to a new index; false on a unique-constraint violation.
    add(s, &natives, &ctx, "indexAdd", |s, ctx, a| {
        let (name, store, index, key) =
            (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_string()?, a.get(3).to_string()?);
        let keys = strings(a.get(4))?;
        let ok = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            let Some(ix) = st.indexes.get_mut(&index) else { return Ok(true) };
            if ix.unique
                && keys.iter().any(|k| {
                    ix.entries.range((k.clone(), String::new())..).next().is_some_and(|(ik, pk)| ik == k && *pk != key)
                })
            {
                return Ok(false);
            }
            for k in &keys {
                ix.entries.insert((k.clone(), key.clone()));
            }
            let id = ix.id;
            if let Some(record) = st.records.get_mut(&key) {
                record.keys.push((id, keys));
            }
            Ok(true)
        })?;
        Ok(s.boolean(ok))
    })?;
    // The key generator's next key and step, or -1 when exhausted.
    add(s, &natives, &ctx, "nextKey", |s, ctx, a| {
        let (name, store, tx) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_number()? as u64);
        let key = ctx.db(&name, |db| {
            let current = db.store(&store)?.current;
            if current > MAX_GENERATED {
                return Ok(-1.0);
            }
            db.log(tx, || Undo::Generator { store: store.clone(), old: current });
            db.store(&store)?.current = after(current);
            Ok(current)
        })?;
        Ok(s.number(key))
    })?;
    // "Possibly update the key generator" for an explicit number key.
    add(s, &natives, &ctx, "bumpKey", |s, ctx, a| {
        let (name, store, tx) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_number()? as u64);
        let key = a.get(3).to_number()?;
        ctx.db(&name, |db| {
            let current = db.store(&store)?.current;
            let value = (key.min(MAX_GENERATED)).floor();
            if value >= current {
                db.log(tx, || Undo::Generator { store: store.clone(), old: current });
                db.store(&store)?.current = after(value);
            }
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    // 0 stored, 1 the key exists (add), 2 a unique index constraint failed.
    add(s, &natives, &ctx, "put", |s, ctx, a| {
        let (name, store, tx, key) =
            (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_number()? as u64, a.get(3).to_string()?);
        let value = Arc::new(encode_message(s, a.get(4), s.undefined())?);
        let mut index_keys = Vec::new();
        if let Some(list) = a.get(5).as_object() {
            let n = list.get_atom("length")?.to_number()? as u32;
            for i in 0..n {
                let row = list.get_index(i)?.as_object().ok_or_else(unavailable)?;
                index_keys.push((row.get_index(0)?.to_string()?, strings(row.get_index(1)?)?));
            }
        }
        let no_overwrite = a.get(6).to_bool();
        let code = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            if no_overwrite && st.records.contains_key(&key) {
                return Ok(1);
            }
            let mut keys = Vec::new();
            for (index, list) in index_keys {
                let Some(ix) = st.indexes.get(&index) else { continue };
                if ix.unique {
                    for k in &list {
                        let taken = ix
                            .entries
                            .range((k.clone(), String::new())..)
                            .take_while(|(ik, _)| ik == k)
                            .any(|(_, pk)| *pk != key);
                        if taken {
                            return Ok(2);
                        }
                    }
                }
                keys.push((ix.id, list));
            }
            let old = st.remove(&key);
            st.insert(key.clone(), Record { value, keys });
            db.log(tx, || Undo::Record { store: store.clone(), key, old });
            Ok(0)
        })?;
        Ok(s.number(code as f64))
    })?;
    // [key, primaryKey, value?] of the first record in range, or null.
    add(s, &natives, &ctx, "getRecord", |s, ctx, a| {
        let (name, store, index) = (a.get(0).to_string()?, a.get(1).to_string()?, opt_string(a.get(2))?);
        let range = range_at(&a, 3)?;
        let want = a.get(7).to_bool();
        let found = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            let src = match &index {
                Some(i) => Source::Index(&st.indexes.get(i).ok_or_else(|| unknown(i))?.entries),
                None => Source::Store(&st.records),
            };
            let pair = seek(&src, &range, Direction::Next, None, None, 1);
            Ok(pair.map(|(k, p)| {
                let value = want.then(|| st.records.get(&p).map(|r| r.value.clone())).flatten();
                (k, p, value)
            }))
        })?;
        let Some((k, p, value)) = found else { return Ok(s.null()) };
        let value = match value {
            Some(v) => decode_message(s, &v)?,
            None => s.undefined(),
        };
        Ok(s.array(&[s.string(&k), s.string(&p), value])?.value())
    })?;
    // Flat [key, primaryKey, value?, ...] in `dir` order, at most `count` (0: all).
    add(s, &natives, &ctx, "getAll", |s, ctx, a| {
        let (name, store, index) = (a.get(0).to_string()?, a.get(1).to_string()?, opt_string(a.get(2))?);
        let range = range_at(&a, 3)?;
        let count = a.get(7).to_number()? as u64;
        let dir = Direction::from(a.get(8).to_number()?);
        let want = a.get(9).to_bool();
        let rows = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            let src = match &index {
                Some(i) => Source::Index(&st.indexes.get(i).ok_or_else(|| unknown(i))?.entries),
                None => Source::Store(&st.records),
            };
            let mut rows = Vec::new();
            let mut position = None;
            while count == 0 || (rows.len() as u64) < count {
                let Some(pair) = seek(&src, &range, dir, position.take(), None, 1) else { break };
                let value = want.then(|| st.records.get(&pair.1).map(|r| r.value.clone())).flatten();
                position = Some(pair.clone());
                rows.push((pair, value));
            }
            Ok(rows)
        })?;
        let mut out = RootedVec::new();
        for ((k, p), value) in rows {
            out.push(s, s.string(&k));
            out.push(s, s.string(&p));
            let value = match value {
                Some(v) => decode_message(s, &v)?,
                None => s.undefined(),
            };
            out.push(s, value);
        }
        Ok(s.array(&out.iter(s).collect::<Vec<_>>())?.value())
    })?;
    add(s, &natives, &ctx, "count", |s, ctx, a| {
        let (name, store, index) = (a.get(0).to_string()?, a.get(1).to_string()?, opt_string(a.get(2))?);
        let range = range_at(&a, 3)?;
        let n = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            Ok(match &index {
                Some(i) => {
                    let ix = st.indexes.get(i).ok_or_else(|| unknown(i))?;
                    ix.entries
                        .range((range.lower_bound(), Bound::Unbounded))
                        .take_while(|(k, _)| range.contains(k))
                        .count()
                }
                None => st
                    .records
                    .range::<String, _>((Source::key_bound(&range.lower_bound(), false), Bound::Unbounded))
                    .take_while(|(k, _)| range.contains(k))
                    .count(),
            })
        })?;
        Ok(s.number(n as f64))
    })?;
    add(s, &natives, &ctx, "deleteRange", |s, ctx, a| {
        let (name, store, tx) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_number()? as u64);
        let range = range_at(&a, 3)?;
        ctx.db(&name, |db| {
            let st = db.store(&store)?;
            let keys: Vec<String> = st
                .records
                .range::<String, _>((Source::key_bound(&range.lower_bound(), false), Bound::Unbounded))
                .map(|(k, _)| k)
                .take_while(|k| range.contains(k))
                .cloned()
                .collect();
            for key in keys {
                let old = db.store(&store)?.remove(&key);
                db.log(tx, || Undo::Record { store: store.clone(), key, old });
            }
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    add(s, &natives, &ctx, "clear", |s, ctx, a| {
        let (name, store, tx) = (a.get(0).to_string()?, a.get(1).to_string()?, a.get(2).to_number()? as u64);
        ctx.db(&name, |db| {
            let st = db.store(&store)?;
            // The schema without the data (cloning the records only to drop them is O(n)).
            let indexes = st
                .indexes
                .iter()
                .map(|(name, ix)| {
                    (name.clone(), Index { entries: BTreeSet::new(), key_path: ix.key_path.clone(), ..*ix })
                })
                .collect();
            let emptied = Store {
                key_path: st.key_path.clone(),
                auto_increment: st.auto_increment,
                current: st.current,
                records: BTreeMap::new(),
                indexes,
            };
            let old = std::mem::replace(st, emptied);
            db.log(tx, || Undo::Store { name: store.clone(), old: Box::new(old) });
            Ok(())
        })?;
        Ok(s.undefined())
    })?;
    // Cursor iteration: [key, primaryKey, value?] or null.
    add(s, &natives, &ctx, "seek", |s, ctx, a| {
        let (name, store, index) = (a.get(0).to_string()?, a.get(1).to_string()?, opt_string(a.get(2))?);
        let range = range_at(&a, 3)?;
        let dir = Direction::from(a.get(7).to_number()?);
        let position = match (opt_string(a.get(8))?, opt_string(a.get(9))?) {
            (Some(k), Some(p)) => Some((k, p)),
            (Some(k), None) => Some((k.clone(), k)),
            _ => None,
        };
        let target = opt_string(a.get(10))?.map(|k| Ok::<_, JsError>((k, opt_string(a.get(11))?))).transpose()?;
        let count = a.get(12).to_number()? as u32;
        let want = a.get(13).to_bool();
        let found = ctx.db(&name, |db| {
            let st = db.store(&store)?;
            let src = match &index {
                Some(i) => Source::Index(&st.indexes.get(i).ok_or_else(|| unknown(i))?.entries),
                None => Source::Store(&st.records),
            };
            let pair = seek(&src, &range, dir, position, target, count);
            Ok(pair.map(|(k, p)| {
                let value = want.then(|| st.records.get(&p).map(|r| r.value.clone())).flatten();
                (k, p, value)
            }))
        })?;
        let Some((k, p, value)) = found else { return Ok(s.null()) };
        let value = match value {
            Some(v) => decode_message(s, &v)?,
            None => s.undefined(),
        };
        Ok(s.array(&[s.string(&k), s.string(&p), value])?.value())
    })?;
    Ok(natives)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> BTreeSet<(String, String)> {
        list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
    }

    fn all() -> Range {
        Range { lower: None, upper: None, lower_open: false, upper_open: false }
    }

    fn walk(src: &Source<'_>, range: &Range, dir: Direction) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let mut position = None;
        while let Some(p) = seek(src, range, dir, position.take(), None, 1) {
            position = Some(p.clone());
            out.push(p);
        }
        out
    }

    #[test]
    fn index_iteration_follows_direction_and_uniqueness() {
        let set = pairs(&[("a", "1"), ("a", "2"), ("b", "3"), ("c", "4"), ("c", "5")]);
        let src = Source::Index(&set);
        let keys =
            |v: Vec<(String, String)>| v.into_iter().map(|(k, p)| format!("{k}{p}")).collect::<Vec<_>>().join(",");
        assert_eq!(keys(walk(&src, &all(), Direction::Next)), "a1,a2,b3,c4,c5");
        assert_eq!(keys(walk(&src, &all(), Direction::NextUnique)), "a1,b3,c4");
        assert_eq!(keys(walk(&src, &all(), Direction::Prev)), "c5,c4,b3,a2,a1");
        assert_eq!(keys(walk(&src, &all(), Direction::PrevUnique)), "c4,b3,a1");
        let open = Range { lower: Some("a".into()), upper: Some("c".into()), lower_open: true, upper_open: true };
        assert_eq!(keys(walk(&src, &open, Direction::Next)), "b3");
        let closed = Range { lower: Some("a".into()), upper: Some("c".into()), lower_open: false, upper_open: false };
        assert_eq!(keys(walk(&src, &closed, Direction::Prev)), "c5,c4,b3,a2,a1");
        // continuePrimaryKey and advance.
        let hit = seek(
            &src,
            &all(),
            Direction::Next,
            Some(("a".into(), "1".into())),
            Some(("c".into(), Some("5".into()))),
            1,
        );
        assert_eq!(hit, Some(("c".into(), "5".into())));
        assert_eq!(seek(&src, &all(), Direction::Next, None, None, 3), Some(("b".into(), "3".into())));
        assert_eq!(
            seek(&src, &all(), Direction::Prev, None, Some(("b".into(), None)), 1),
            Some(("b".into(), "3".into()))
        );
    }

    #[test]
    fn store_iteration_uses_keys_as_pairs() {
        let mut map = BTreeMap::new();
        for k in ["1", "2", "3"] {
            map.insert(k.to_string(), Record { value: Arc::new(WireMessage::empty()), keys: Vec::new() });
        }
        let src = Source::Store(&map);
        let got: Vec<String> = walk(&src, &all(), Direction::Prev).into_iter().map(|(k, _)| k).collect();
        assert_eq!(got, ["3", "2", "1"]);
        let r = Range { lower: Some("1".into()), upper: Some("3".into()), lower_open: true, upper_open: false };
        let got: Vec<String> = walk(&src, &r, Direction::Next).into_iter().map(|(k, _)| k).collect();
        assert_eq!(got, ["2", "3"]);
        assert_eq!(
            seek(&src, &all(), Direction::Next, None, Some(("2".into(), None)), 1).map(|p| p.0),
            Some("2".into())
        );
    }

    #[test]
    fn catalog_snapshots_are_origin_and_context_scoped() {
        let context = Namespace::default();
        let shared = context.clone();
        let isolated = Namespace::default();
        context
            .lock()
            .origins
            .entry("http://one.test".into())
            .or_default()
            .insert("one".into(), Db { version: 3, ..Default::default() });
        let snapshot = shared.databases("http://one.test");
        assert_eq!((&*snapshot[0].name, snapshot[0].version), ("one", 3));
        assert!(shared.databases("http://two.test").is_empty());
        assert!(isolated.databases("http://one.test").is_empty());
    }
}

/// This module's part of [`crate::thread_state::reset_thread`]: what of its
/// per-thread state still holds something, if any does.
pub(crate) fn thread_leftover() -> Option<&'static str> {
    REALMS.with(|r| !r.borrow().is_empty()).then_some("indexeddb::REALMS")
}
