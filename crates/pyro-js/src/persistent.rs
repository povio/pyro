//! GC roots for values stored outside the native stack.

use crate::engine::{self, EngineInner};
use crate::realm::Scope;
use crate::value::Value;
use pyro_jsc_sys as sys;
use std::rc::{Rc, Weak};

/// A strong, GC-protected reference to a JavaScript value (`JSValueProtect`).
///
/// Safe to drop at any time: after engine shutdown (which destroys the VM even
/// if `Persistent`s remain, e.g. inside closure payloads) it is inert. Clone re-protects
/// (JSC protection is counted). `!Send`: it may only be used and dropped on the
/// engine thread. Reachability through a `Persistent` is invisible to the GC's
/// cycle detection: do not let a value protected by a native object reference
/// that native object's wrapper, or both leak; use hidden properties for
/// wrapper-owned JS state instead.
pub struct Persistent {
    raw: sys::JSValueRef,
    /// Weak: a Persistent (e.g. captured by a closure payload) must not keep
    /// the engine, and so the VM it lives in, alive.
    engine: Weak<EngineInner>,
    root: sys::JSContextRef,
}

impl Persistent {
    pub fn new(scope: &Scope<'_>, value: Value<'_>) -> Persistent {
        let engine = scope.engine_inner();
        let root = engine.root_ctx();
        // SAFETY: value is live in this scope; root ctx belongs to the same VM.
        unsafe { sys::JSValueProtect(root, value.raw) };
        Persistent { raw: value.raw, engine: Rc::downgrade(engine), root }
    }

    /// Protects a raw value using this thread's engine; `None` without one.
    ///
    /// # Safety
    /// `raw` must be a live value of this thread's engine.
    pub(crate) unsafe fn from_raw(ctx: sys::JSContextRef, raw: sys::JSValueRef) -> Option<Persistent> {
        let engine = engine::engine_for_ctx(ctx)?;
        let root = engine.root_ctx();
        unsafe { sys::JSValueProtect(root, raw) };
        Some(Persistent { raw, engine: Rc::downgrade(&engine), root })
    }

    /// The value, usable within `scope`.
    pub fn get<'s>(&self, scope: &Scope<'s>) -> Value<'s> {
        debug_assert!(
            std::ptr::eq(self.engine.as_ptr(), Rc::as_ptr(scope.engine_inner())),
            "Persistent used with another engine"
        );
        // SAFETY: protected value of the same VM; the scope's ctx is live.
        unsafe { Value::from_raw(scope.ctx, self.raw) }
    }

    pub fn as_raw(&self) -> sys::JSValueRef {
        self.raw
    }

    /// Identity comparison (same JS value bits, e.g. same object).
    pub fn ptr_eq(&self, other: &Persistent) -> bool {
        self.raw == other.raw
    }
}

impl Clone for Persistent {
    fn clone(&self) -> Self {
        if self.vm_alive() {
            unsafe { sys::JSValueProtect(self.root, self.raw) };
        }
        Persistent { raw: self.raw, engine: self.engine.clone(), root: self.root }
    }
}

impl Drop for Persistent {
    fn drop(&mut self) {
        // Never called from inside a JSC finalizer: wrapper payloads are dropped
        // at safe points (see class::drain_finalized), so unprotect is legal
        // here. After engine shutdown the VM is gone and nothing is protected.
        if self.vm_alive() {
            unsafe { sys::JSValueUnprotect(self.root, self.raw) };
        }
    }
}

impl Persistent {
    fn vm_alive(&self) -> bool {
        self.engine.upgrade().is_some_and(|e| !e.is_shut_down())
    }
}

impl std::fmt::Debug for Persistent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Persistent({:p})", self.raw)
    }
}

/// A value kept alive, like a [`Persistent`], for as long as the handle
/// lives, but without a root of its own: the engine keeps every such value
/// in one JavaScript array (protected once) at the handle's index, and the
/// handle holds the value's bits for reads that never enter JSC.
///
/// For many small values that live as long as host objects do (a string
/// beside each of 100k wrappers): a `Persistent` is an entry in JSC's
/// protect table, which every collection, eden ones included, walks as
/// roots, and which with its handle costs a few hundred bytes per value.
/// The array is one old object: an eden collection visits it only after it
/// was written, and a value costs its 8-byte slot. A freed slot is reused
/// by the next handle; the array never shrinks.
///
/// The values must not refer back to whatever owns the handle (a wrapper's
/// payload): as with a `Persistent`, the collector cannot see that cycle.
/// Strings cannot.
pub struct HeldValue {
    raw: sys::JSValueRef,
    index: u32,
    engine: Weak<EngineInner>,
}

/// The engine's array of [`HeldValue`]s and its free slots.
#[derive(Default)]
pub(crate) struct HeldValues {
    array: Option<sys::JSObjectRef>,
    len: u32,
    free: Vec<u32>,
}

impl HeldValues {
    /// With no value held: drops the array and its protection, as before the
    /// first value. `false`, changing nothing, while one is held.
    pub(crate) fn reset(&mut self, root: sys::JSContextRef) -> bool {
        if self.free.len() != self.len as usize {
            return false;
        }
        if let Some(array) = self.array.take() {
            // SAFETY: the array was protected in the live root context.
            unsafe { sys::JSValueUnprotect(root, array as sys::JSValueRef) };
        }
        self.len = 0;
        self.free.clear();
        true
    }
}

impl HeldValue {
    pub fn new(scope: &Scope<'_>, value: Value<'_>) -> HeldValue {
        let engine = scope.engine_inner();
        let root = engine.root_ctx();
        let mut held = engine.held.borrow_mut();
        // SAFETY: the root context is live; the array is created in it (whose
        // prototypes no page script can reach, so the store below runs no
        // script) and protected for the engine's lifetime.
        let array = *held.array.get_or_insert_with(|| unsafe {
            let array = sys::JSObjectMakeArray(root, 0, std::ptr::null(), std::ptr::null_mut());
            sys::JSValueProtect(root, array as sys::JSValueRef);
            array
        });
        let index = held.free.pop().unwrap_or_else(|| {
            held.len += 1;
            held.len - 1
        });
        // SAFETY: as above; `value` is live in this scope, of the same VM.
        unsafe { sys::JSObjectSetPropertyAtIndex(root, array, index, value.raw, std::ptr::null_mut()) };
        HeldValue { raw: value.raw, index, engine: Rc::downgrade(engine) }
    }

    /// The value, usable within `scope`.
    #[inline]
    pub fn get<'s>(&self, scope: &Scope<'s>) -> Value<'s> {
        debug_assert!(
            std::ptr::eq(self.engine.as_ptr(), Rc::as_ptr(scope.engine_inner())),
            "HeldValue used with another engine"
        );
        // SAFETY: held by the engine's array, same VM; the scope's ctx is live.
        unsafe { Value::from_raw(scope.ctx, self.raw) }
    }
}

impl Drop for HeldValue {
    fn drop(&mut self) {
        // Dropped at safe points, as `Persistent`s are; after engine shutdown
        // the VM is gone and nothing is held.
        let Some(engine) = self.engine.upgrade().filter(|e| !e.is_shut_down()) else { return };
        let Ok(mut held) = engine.held.try_borrow_mut() else { return };
        let Some(array) = held.array else { return };
        let root = engine.root_ctx();
        // SAFETY: the engine's live array; undefined keeps it dense.
        unsafe {
            sys::JSObjectSetPropertyAtIndex(
                root,
                array,
                self.index,
                sys::JSValueMakeUndefined(root),
                std::ptr::null_mut(),
            )
        };
        held.free.push(self.index);
    }
}

impl std::fmt::Debug for HeldValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HeldValue({:p} at {})", self.raw, self.index)
    }
}

/// A growable list of rooted values for temporary heap collections.
#[derive(Default, Clone, Debug)]
pub struct RootedVec {
    items: Vec<Persistent>,
}

impl RootedVec {
    pub fn new() -> RootedVec {
        RootedVec { items: Vec::new() }
    }
    pub fn push(&mut self, scope: &Scope<'_>, v: Value<'_>) {
        self.items.push(Persistent::new(scope, v));
    }
    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn get<'s>(&self, scope: &Scope<'s>, i: usize) -> Option<Value<'s>> {
        self.items.get(i).map(|p| p.get(scope))
    }
    pub fn iter<'a, 's>(&'a self, scope: &'a Scope<'s>) -> impl Iterator<Item = Value<'s>> + 'a {
        self.items.iter().map(move |p| p.get(scope))
    }

    /// Raw values for a call; every element stays protected by `self`.
    pub(crate) fn raw(&self) -> Vec<sys::JSValueRef> {
        self.items.iter().map(|p| p.as_raw()).collect()
    }
}
