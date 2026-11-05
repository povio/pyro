//! One JavaScript string per interned host string (an atom), per realm, held
//! weakly.
//!
//! Interned host strings (the DOM's atoms: element ids, class values, local
//! names, namespaces) have an identity: two atoms with the same bits have the
//! same text for as long as either lives. A getter returning one used to make
//! a new JSString on every read (a UTF-8 conversion, a `StringImpl` and a GC
//! cell, garbage right after). The realm keeps, per identity, a weak handle to
//! the string it made on the first read; later reads return that string.
//! Nothing is invalidated: a changed id is another atom.
//!
//! An identity that is the text itself or an index into a fixed set (inline
//! and static atoms) names its text for good. One that is an address (a
//! dynamic atom's entry) can name another text once the host frees it, so
//! its entry keeps a copy of the text, compared on each read. Holding a clone
//! of the atom instead was measured as a loss: a clone kept until the string
//! is collected keeps every unique id a page read since the last collection
//! interned, and string_cache's global set (4096 chained buckets) then made
//! each later `setAttribute('id', ..)` several times slower.
//!
//! Ids are mostly unique, so the strings are weak: the collector reclaims the
//! ones nothing references, and the first insert after a collection drops
//! such entries (never a read). The table learns of a collection from a weak
//! handle to an object nothing references, made at each sweep: JSC's C API
//! has no collection callback.
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

use rustc_hash::FxHashMap;

use crate::realm::Scope;
use crate::sys;
use crate::value::Value;

/// An interned string's identity: its bits, and which set of interned
/// strings it is from (two sets may give the same bits to different text).
pub type InternedId = (u64, usize);

/// Capacity a sweep keeps however few entries are left.
const KEEP_CAPACITY: usize = 256;

type WeakFns = (sys::spi::JSWeakCreateFn, sys::spi::JSWeakReleaseFn, sys::spi::JSWeakGetObjectFn);

pub(crate) struct InternedStrings {
    map: FxHashMap<InternedId, Entry>,
    /// Weak handle to an object made unreferenced at the last sweep (null
    /// before the first insert): once it is dead, a collection has run.
    canary: sys::JSWeakRef,
    /// JSC's weak handle entry points; `None` (no table) without them.
    fns: Option<WeakFns>,
}

struct Entry {
    weak: sys::JSWeakRef,
    /// The text of a reusable identity, compared on each read.
    text: Option<Box<str>>,
}

impl InternedStrings {
    pub(crate) fn new() -> Self {
        let fns = if cfg!(target_pointer_width = "64") { sys::spi::get().weak() } else { None };
        InternedStrings { map: FxHashMap::default(), canary: std::ptr::null_mut(), fns }
    }

    /// Whether a collection may have reclaimed strings since the last sweep.
    fn collected(&self, get: sys::spi::JSWeakGetObjectFn) -> bool {
        // SAFETY: the table owns the canary handle.
        self.canary.is_null() || unsafe { get(self.canary) }.is_null()
    }

    /// Drops the entries whose string the collector reclaimed, and makes
    /// the canary that tells of the next collection.
    #[inline(never)]
    fn sweep(&mut self, scope: &Scope<'_>, group: sys::JSContextGroupRef) {
        let Some((create, release, get)) = self.fns else { return };
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
        // An object only the handle refers to (once this frame is gone):
        // the next collection, eden or full, reclaims it.
        // SAFETY: a live context; the old handle is the table's.
        unsafe {
            if !self.canary.is_null() {
                release(group, self.canary);
            }
            self.canary = create(group, scope.object().as_raw());
        }
    }

    /// Releases every handle (the realm is being disposed; the VM is live).
    pub(crate) fn clear(&mut self, group: sys::JSContextGroupRef) {
        let Some((_, release, _)) = self.fns else { return };
        for (_, e) in self.map.drain() {
            // SAFETY: owned by the table; the VM outlives the realm.
            unsafe { release(group, e.weak) };
        }
        let canary = std::mem::replace(&mut self.canary, std::ptr::null_mut());
        if !canary.is_null() {
            // SAFETY: as above.
            unsafe { release(group, canary) };
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.map.len()
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
            if e.text.as_deref().is_none_or(|t| t == text) {
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
        let mut table = self.realm.interned.borrow_mut();
        let Some((create, release, get)) = table.fns else { return v };
        let group = self.engine_inner().group();
        // SAFETY: a live string cell of this VM (see the module docs).
        let weak = unsafe { create(group, v.as_raw() as sys::JSObjectRef) };
        let entry = Entry { weak, text: check.map(Box::from) };
        if let Some(e) = table.map.get_mut(&id) {
            // Its string was reclaimed, or `id` names another text now: this
            // string takes its place.
            let old = std::mem::replace(e, entry);
            // SAFETY: the table owned the old handle.
            unsafe { release(group, old.weak) };
        } else {
            if table.collected(get) {
                table.sweep(self, group);
            }
            table.map.insert(id, entry);
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
