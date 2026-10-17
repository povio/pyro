//! GC roots for values stored outside the native stack.

use crate::engine::{self, EngineInner};
use crate::realm::Scope;
use crate::value::Value;
use lp_jsc_sys as sys;
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
