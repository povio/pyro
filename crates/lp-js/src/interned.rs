//! One JavaScript string per interned host string (an atom), per realm, held
//! weakly.
//!
//! Interned host strings (the DOM's atoms: element ids, class names, local
//! names, namespaces) have an identity: two atoms with the same bits have the
//! same text for as long as either lives. A getter returning one used to make
//! a new JSString on every read (a UTF-8 conversion, a `StringImpl` and a GC
//! cell, garbage right after). The realm keeps, per identity, a weak handle to
//! the string it made on the first read; later reads return that string.
//! Nothing is invalidated: a changed id is another atom.
//!
//! An identity that is the text itself or an index into a fixed set (inline
//! and static atoms) names its text for good. One that is an address (a
//! dynamic atom's entry) can name another text once the host frees it, and
//! a hash ([`Scope::text_string`], for text with no atom: attribute values)
//! can name two, so such an entry keeps a copy of the text (inline up to
//! [`INLINE_TEXT`] bytes), compared on each read. Holding a
//! clone of the atom instead was measured as a loss: a clone kept until the
//! string is collected keeps every unique id a page read since the last
//! collection interned, and string_cache's global set (4096 chained buckets)
//! then made each later `setAttribute('id', ..)` several times slower.
//!
//! The strings are weak: ids are mostly unique, and the collector reclaims
//! the ones nothing references. The entries of reclaimed strings are swept
//! after a collection, at an insert or at the engine's safe point after each
//! task ([`sweep_realms`]), once the entries added since the last sweep are
//! a quarter of the ones it kept (so a sweep's walk costs a few steps per
//! insert), or every [`SWEEP_EVERY`] collections; never on a read. JSC's C
//! API has no collection callback: [`Collections`] tells of one by a weak
//! handle to an object nothing references.
//!
//! A first read costs two to three plain strings: JSC's `JSWeakCreate` takes
//! the API lock and allocates, and `JSWeakRelease` takes the lock again
//! later (most of a sweep). A value pays off from its third or fourth read
//! while its string lives; a collection between reads reclaims it.
//!
//! The weak handles are JSC's `JSWeak`, made for objects: `JSWeakCreate`
//! keeps the cell in a `Weak<JSObject>`, whose machinery (`WeakSet`
//! allocation, reaping an unmarked cell at the end of a collection) is the
//! same for every cell, and `toJS(JSObjectRef)` checks only that the cell has
//! a method table, which a string has (without `EXTRA_INTEGRITY_CHECKS`, off
//! in release builds of both the patched and the system framework).
//! `JSWeakGetObject` returns the cell unchanged; on 64-bit, a string's
//! `JSValueRef` is its cell pointer.

use std::borrow::Cow;
use std::cell::Cell;
use std::hash::Hasher;

use rustc_hash::FxHashMap;

use crate::engine::EngineInner;
use crate::realm::Scope;
use crate::sys;
use crate::value::Value;

/// An interned string's identity: its bits, and which set of interned
/// strings it is from (two sets may give the same bits to different text).
/// Set `usize::MAX` is [`Scope::text_string`]'s.
pub type InternedId = (u64, usize);

/// The set of [`Scope::text_string`]'s identities (their text's hash).
const TEXT_SET: usize = usize::MAX;

/// Capacity a sweep keeps however few entries are left.
const KEEP_CAPACITY: usize = 256;

/// A table not swept for this many collections is swept at the next safe
/// point (entries whose strings died with no insert since).
const SWEEP_EVERY: u32 = 8;

/// Texts up to this many bytes are copied into the entry itself.
const INLINE_TEXT: usize = 22;

/// Longer texts are no [`Scope::text_string`] (a `data:` URL, say): its copy
/// for the check would double what such a string holds.
const MAX_TEXT: usize = 1024;

type WeakFns = (sys::spi::JSWeakCreateFn, sys::spi::JSWeakReleaseFn, sys::spi::JSWeakGetObjectFn);

pub(crate) struct InternedStrings {
    map: FxHashMap<InternedId, Entry>,
    /// [`Collections::count`] at the last sweep.
    swept_at: u32,
    /// Entries the last sweep kept, and entries inserted since.
    kept: usize,
    added: usize,
    /// JSC's weak handle entry points; `None` (no table) without them.
    fns: Option<WeakFns>,
}

struct Entry {
    weak: sys::JSWeakRef,
    /// The text of a reusable identity, compared on each read.
    text: Option<Text>,
}

/// A copy of an entry's text: in the entry up to [`INLINE_TEXT`] bytes
/// (most ids), else boxed.
enum Text {
    Inline(u8, [u8; INLINE_TEXT]),
    Boxed(Box<str>),
}

impl Text {
    fn new(text: &str) -> Text {
        if text.len() <= INLINE_TEXT {
            let mut bytes = [0; INLINE_TEXT];
            bytes[..text.len()].copy_from_slice(text.as_bytes());
            Text::Inline(text.len() as u8, bytes)
        } else {
            Text::Boxed(text.into())
        }
    }
    #[inline]
    fn is(&self, text: &str) -> bool {
        match self {
            Text::Inline(n, bytes) => bytes[..*n as usize] == *text.as_bytes(),
            Text::Boxed(b) => **b == *text,
        }
    }
}

impl InternedStrings {
    pub(crate) fn new() -> Self {
        let fns = if cfg!(target_pointer_width = "64") { sys::spi::get().weak() } else { None };
        InternedStrings { map: FxHashMap::default(), swept_at: 0, kept: 0, added: 0, fns }
    }

    /// Whether a sweep is worth its walk, `collections` being the engine's
    /// count now: only a collection reclaims strings, and the walk visits
    /// every entry.
    #[inline]
    fn sweep_due(&self, collections: u32) -> bool {
        collections != self.swept_at
            && ((self.added > 0 && 4 * self.added >= self.kept)
                || collections.wrapping_sub(self.swept_at) >= SWEEP_EVERY)
    }

    /// Drops the entries whose string the collector reclaimed.
    #[inline(never)]
    fn sweep(&mut self, group: sys::JSContextGroupRef, collections: u32) {
        let Some((_, release, get)) = self.fns else { return };
        self.map.retain(|_, e| {
            // SAFETY: the table owns its handles, released once here.
            unsafe {
                let live = !get(e.weak).is_null();
                if !live {
                    release(group, e.weak);
                }
                live
            }
        });
        if self.map.capacity() > 4 * self.map.len().max(KEEP_CAPACITY) {
            self.map.shrink_to(2 * self.map.len().max(KEEP_CAPACITY));
        }
        (self.swept_at, self.kept, self.added) = (collections, self.map.len(), 0);
    }

    /// Releases every handle (the realm is being disposed; the VM is live).
    pub(crate) fn clear(&mut self, group: sys::JSContextGroupRef) {
        let Some((_, release, _)) = self.fns else { return };
        for (_, e) in self.map.drain() {
            // SAFETY: owned by the table; the VM outlives the realm.
            unsafe { release(group, e.weak) };
        }
        (self.kept, self.added) = (0, 0);
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
    }
}

/// Counts the engine's garbage collections, as far as they can be seen: a
/// weak handle to an object nothing references dies at the next one (eden
/// or full). Each count made after a collection makes a new such object.
pub(crate) struct Collections {
    canary: Cell<sys::JSWeakRef>,
    count: Cell<u32>,
    /// The count when [`sweep_realms`] last looked at the realms.
    looked_at: Cell<u32>,
    /// A realm of the engine has made an entry: until then no canary.
    used: Cell<bool>,
}

impl Collections {
    pub(crate) fn new() -> Self {
        Collections {
            canary: Cell::new(std::ptr::null_mut()),
            count: Cell::new(0),
            looked_at: Cell::new(0),
            used: Cell::new(false),
        }
    }

    /// Collections seen so far: one more than at the last call when one has
    /// run since (however many did).
    pub(crate) fn count(&self, engine: &EngineInner) -> u32 {
        let Some((create, release, get)) = sys::spi::get().weak() else { return 0 };
        let canary = self.canary.get();
        // SAFETY: the canary handle is ours.
        if !canary.is_null() && !unsafe { get(canary) }.is_null() {
            return self.count.get();
        }
        // SAFETY: the engine is live (callers check); the old handle is ours.
        unsafe {
            if !canary.is_null() {
                release(engine.group(), canary);
            }
            // An object only the handle refers to once this frame is gone.
            let object = sys::JSObjectMake(engine.root_ctx(), std::ptr::null_mut(), std::ptr::null_mut());
            self.canary.set(create(engine.group(), object));
        }
        self.count.set(self.count.get().wrapping_add(1));
        self.count.get()
    }

    /// Releases the canary (engine shutdown, before the VM goes).
    pub(crate) fn release(&self, group: sys::JSContextGroupRef) {
        let canary = self.canary.replace(std::ptr::null_mut());
        if let (false, Some((_, release, _))) = (canary.is_null(), sys::spi::get().weak()) {
            // SAFETY: ours, and the VM is still alive.
            unsafe { release(group, canary) };
        }
    }
}

/// Sweeps the tables of the engine's live realms that are due, once per
/// collection seen ([`crate::Engine::drain_finalized`], a safe point: no
/// table is borrowed). `force`: every table, now (an idle full collection).
pub(crate) fn sweep_realms(engine: &EngineInner, force: bool) {
    if engine.is_shut_down() || !engine.collections.used.get() {
        return;
    }
    let collections = engine.collections.count(engine);
    if !force && engine.collections.looked_at.replace(collections) == collections {
        return;
    }
    for realm in engine.live_realms() {
        if let Ok(mut table) = realm.interned.try_borrow_mut() {
            if !table.map.is_empty() && (force || table.sweep_due(collections)) {
                table.sweep(engine.group(), collections);
            }
        }
    }
}

impl<'s> Scope<'s> {
    /// The realm's JavaScript string for an interned host string: `id`
    /// names `text` (as an atom's bits name its text). The first read makes
    /// the string and keeps a weak handle to it; later reads return the
    /// same string until the collector reclaims it (nothing referenced it),
    /// and then a new one.
    ///
    /// `reusable`: the host may give `id` to another text once this one is
    /// freed (a dynamic atom's bits are its entry's address); the table then
    /// keeps a copy of `text` and returns the string only while it matches.
    #[inline]
    pub fn interned_string(&self, id: InternedId, text: &str, reusable: bool) -> Value<'s> {
        self.interned_string_with(id, text, reusable, || text.into())
    }

    /// The realm's JavaScript string for `text`, by its content: one weakly
    /// held string per distinct text (an attribute value, say), made on the
    /// first read like [`Scope::interned_string`]'s. The identity is the
    /// text's hash and the text is compared on each read: two texts with
    /// one hash take the entry in turn. A text over [`MAX_TEXT`] bytes is a
    /// new string each time.
    ///
    /// Out of line: its callers (`getAttribute`) inline an atom's probe of
    /// the table already, and one copy of this one serves them all.
    #[inline(never)]
    pub fn text_string(&self, text: &str) -> Value<'s> {
        if text.len() > MAX_TEXT {
            return self.string(text);
        }
        let mut hash = rustc_hash::FxHasher::default();
        hash.write(text.as_bytes());
        self.interned_string((hash.finish(), TEXT_SET), text, true)
    }

    /// [`Scope::interned_string`] for a string derived from the interned
    /// text (an uppercased name, say): `make` runs only when the string is
    /// made, and `text` is the text `id` names.
    #[inline]
    pub fn interned_string_with<'t>(
        &self,
        id: InternedId,
        text: &str,
        reusable: bool,
        make: impl FnOnce() -> Cow<'t, str>,
    ) -> Value<'s> {
        let table = self.realm.interned.borrow();
        if let (Some(e), Some((_, _, get))) = (table.map.get(&id), table.fns) {
            if e.text.as_ref().is_none_or(|t| t.is(text)) {
                // SAFETY: a handle owned by the table; a live result is a
                // string of this VM, rooted from here on by the caller's stack.
                let v = unsafe { get(e.weak) };
                if !v.is_null() {
                    return unsafe { Value::from_raw(self.ctx, v as sys::JSValueRef) };
                }
            }
        }
        drop(table);
        self.interned_string_new(id, reusable.then_some(text), &make())
    }

    #[inline(never)]
    fn interned_string_new(&self, id: InternedId, check: Option<&str>, string: &str) -> Value<'s> {
        let v = self.string(string);
        let engine = self.engine_inner();
        let mut table = self.realm.interned.borrow_mut();
        let Some((create, release, _)) = table.fns else { return v };
        let group = engine.group();
        // SAFETY: a live string cell of this VM (see the module docs).
        let weak = unsafe { create(group, v.as_raw() as sys::JSObjectRef) };
        let entry = Entry { weak, text: check.map(Text::new) };
        match table.map.entry(id) {
            std::collections::hash_map::Entry::Occupied(mut e) => {
                // Its string was reclaimed, or `id` names another text now:
                // this string takes its place.
                let old = e.insert(entry);
                // SAFETY: the table owned the old handle.
                unsafe { release(group, old.weak) };
            }
            std::collections::hash_map::Entry::Vacant(e) => {
                e.insert(entry);
                table.added += 1;
                engine.collections.used.set(true);
                // The cheap half first: the count looks at the canary.
                if 4 * table.added >= table.kept {
                    let collections = engine.collections.count(engine);
                    if table.sweep_due(collections) {
                        table.sweep(group, collections);
                    }
                }
            }
        }
        v
    }
}

impl crate::Realm {
    /// Entries in the realm's interned-string table (tests).
    #[doc(hidden)]
    pub fn interned_string_count(&self) -> usize {
        self.inner.interned.borrow().len()
    }
}
