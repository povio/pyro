//! GC roots for values stored outside the native stack.

use crate::engine::{self, EngineInner};
use crate::realm::Scope;
use crate::value::Value;
use lp_jsc_sys as sys;
use std::rc::Rc;

/// A strong, GC-protected reference to a JavaScript value (`JSValueProtect`).
///
/// Keeps the engine (VM) alive, so it is always safe to drop. Clone re-protects
/// (JSC protection is counted). `!Send`: it may only be used and dropped on the
/// engine thread. Reachability through a `Persistent` is invisible to the GC's
/// cycle detection: do not let a value protected by a native object reference
/// that native object's wrapper, or both leak; use hidden properties for
/// wrapper-owned JS state instead.
pub struct Persistent {
    raw: sys::JSValueRef,
    engine: Rc<EngineInner>,
}

impl Persistent {
    pub fn new(scope: &Scope<'_>, value: Value<'_>) -> Persistent {
        let engine = scope.engine_inner().clone();
        // SAFETY: value is live in this scope; root ctx belongs to the same VM.
        unsafe { sys::JSValueProtect(engine.root_ctx(), value.raw) };
        Persistent { raw: value.raw, engine }
    }

    /// Protects a raw value using this thread's engine; `None` without one.
    ///
    /// # Safety
    /// `raw` must be a live value of this thread's engine.
    pub(crate) unsafe fn from_raw(_ctx: sys::JSContextRef, raw: sys::JSValueRef) -> Option<Persistent> {
        let engine = engine::current_inner()?;
        unsafe { sys::JSValueProtect(engine.root_ctx(), raw) };
        Some(Persistent { raw, engine })
    }

    /// The value, usable within `scope`.
    pub fn get<'s>(&self, scope: &Scope<'s>) -> Value<'s> {
        debug_assert!(Rc::ptr_eq(&self.engine, scope.engine_inner()), "Persistent used with another engine");
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
        unsafe { sys::JSValueProtect(self.engine.root_ctx(), self.raw) };
        Persistent { raw: self.raw, engine: self.engine.clone() }
    }
}

impl Drop for Persistent {
    fn drop(&mut self) {
        // Never called from inside a JSC finalizer: wrapper payloads are dropped
        // at safe points (see class::drain_finalized), so unprotect is legal here.
        unsafe { sys::JSValueUnprotect(self.engine.root_ctx(), self.raw) };
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
}
