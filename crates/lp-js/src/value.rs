//! Stack-scoped JavaScript values.
//!
//! # GC rooting rule
//! JavaScriptCore scans the native stack (and registers) of the thread holding
//! the VM conservatively, so a [`Value`] held in a local variable or argument is
//! kept alive. Values stored anywhere on the heap (`Vec`, `Box`, struct fields
//! reached through pointers) are NOT scanned: store a [`crate::Persistent`]
//! instead, or a [`crate::RootedVec`] for temporary collections. The `'s`
//! lifetime ties a value to the [`Scope`] that produced it so it cannot outlive
//! the realm borrow, and all value types are `!Send`.

use crate::error::{JsError, JsResult};
use crate::realm::Scope;
use crate::string::{latin1_to_string, to_rust_string, utf16_to_string, with_host_units, HostUnits, JsString};
use lp_jsc_sys as sys;
use std::marker::PhantomData;
use std::ptr;

/// A JavaScript value valid within scope `'s` on the engine thread.
#[derive(Clone, Copy)]
pub struct Value<'s> {
    pub(crate) raw: sys::JSValueRef,
    pub(crate) ctx: sys::JSContextRef,
    _scope: PhantomData<&'s *const ()>,
}

/// A JavaScript object valid within scope `'s`.
#[derive(Clone, Copy)]
pub struct Object<'s> {
    pub(crate) raw: sys::JSObjectRef,
    pub(crate) ctx: sys::JSContextRef,
    _scope: PhantomData<&'s *const ()>,
}

/// Coarse JavaScript type (`typeof` plus null).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsType {
    Undefined,
    Null,
    Boolean,
    Number,
    String,
    Object,
    Symbol,
    BigInt,
}

impl<'s> Value<'s> {
    /// # Safety
    /// `raw` must be a live value (rooted per the module rule) and `ctx` a live
    /// context of the current engine thread.
    pub unsafe fn from_raw(ctx: sys::JSContextRef, raw: sys::JSValueRef) -> Value<'s> {
        debug_assert!(!raw.is_null());
        Value { raw, ctx, _scope: PhantomData }
    }

    pub fn as_raw(&self) -> sys::JSValueRef {
        self.raw
    }

    pub fn ctx(&self) -> sys::JSContextRef {
        self.ctx
    }

    pub fn ty(&self) -> JsType {
        // SAFETY: live value/context per construction invariant.
        match unsafe { sys::JSValueGetType(self.ctx, self.raw) } {
            sys::kJSTypeUndefined => JsType::Undefined,
            sys::kJSTypeNull => JsType::Null,
            sys::kJSTypeBoolean => JsType::Boolean,
            sys::kJSTypeNumber => JsType::Number,
            sys::kJSTypeString => JsType::String,
            sys::kJSTypeSymbol => JsType::Symbol,
            sys::kJSTypeBigInt => JsType::BigInt,
            _ => JsType::Object,
        }
    }

    pub fn is_undefined(&self) -> bool {
        unsafe { sys::JSValueIsUndefined(self.ctx, self.raw) }
    }
    pub fn is_null(&self) -> bool {
        unsafe { sys::JSValueIsNull(self.ctx, self.raw) }
    }
    pub fn is_nullish(&self) -> bool {
        self.is_undefined() || self.is_null()
    }
    pub fn is_boolean(&self) -> bool {
        unsafe { sys::JSValueIsBoolean(self.ctx, self.raw) }
    }
    pub fn is_number(&self) -> bool {
        unsafe { sys::JSValueIsNumber(self.ctx, self.raw) }
    }
    pub fn is_string(&self) -> bool {
        unsafe { sys::JSValueIsString(self.ctx, self.raw) }
    }

    /// The characters of a string value, without a copy (patched JSC;
    /// `None` otherwise, or for a non-string): `Latin1` bytes or UTF-16
    /// units, borrowed for as long as this value is. Callers that want a
    /// `String` for anything but a lookup should use `to_string`.
    pub fn string_units(&self) -> Option<StringUnits<'_>> {
        let f = sys::spi::get().value_string_characters?;
        let mut len = 0usize;
        let mut is_8bit = false;
        // SAFETY: a live value on its thread; the pointer refers to the
        // string cell's own buffer, which outlives this borrow of the value.
        let p = unsafe { f(self.ctx, self.raw, &mut len, &mut is_8bit) };
        if p.is_null() {
            return None;
        }
        Some(if is_8bit {
            StringUnits::Latin1(unsafe { std::slice::from_raw_parts(p.cast::<u8>(), len) })
        } else {
            StringUnits::Utf16(unsafe { std::slice::from_raw_parts(p.cast::<u16>(), len) })
        })
    }

    /// [`Value::string_units`] for any caller, inside a native callback or
    /// not: patch 0016's JavaScriptCore reads a flat string with no lock and
    /// no trap check, and takes the API lock only to resolve a rope outside a
    /// callback, where a failure leaves no exception pending. `None` without
    /// that patch.
    fn string_units_anywhere(&self) -> Option<StringUnits<'_>> {
        sys::spi::direct_strings()?;
        self.string_units()
    }
    pub fn is_symbol(&self) -> bool {
        unsafe { sys::JSValueIsSymbol(self.ctx, self.raw) }
    }
    pub fn is_object(&self) -> bool {
        unsafe { sys::JSValueIsObject(self.ctx, self.raw) }
    }
    pub fn is_array(&self) -> bool {
        unsafe { sys::JSValueIsArray(self.ctx, self.raw) }
    }
    pub fn is_date(&self) -> bool {
        unsafe { sys::JSValueIsDate(self.ctx, self.raw) }
    }
    pub fn is_function(&self) -> bool {
        self.as_object().is_some_and(|o| o.is_function())
    }

    /// Tests for a native JSC proxy target without reading properties or invoking
    /// traps. Includes global proxies and may include revoked proxies whose
    /// target is retained. A false result is not a complete cloneability check.
    /// Non-objects return false. Objects require the optional proxy-target SPI.
    pub fn has_proxy_target(&self) -> JsResult<bool> {
        let Some(object) = self.as_object() else { return Ok(false) };
        let get_target = sys::spi::get().get_proxy_target.ok_or(JsError::Unsupported("proxy target inspection"))?;
        // SAFETY: live object on its owning VM thread; SPI only reads its native
        // type/target under the VM lock. The target is neither exposed nor kept.
        Ok(unsafe { !get_target(object.raw).is_null() })
    }

    pub fn as_object(&self) -> Option<Object<'s>> {
        if self.is_object() {
            Some(Object { raw: self.raw as sys::JSObjectRef, ctx: self.ctx, _scope: PhantomData })
        } else {
            None
        }
    }

    /// ToBoolean (never throws).
    pub fn to_bool(&self) -> bool {
        unsafe { sys::JSValueToBoolean(self.ctx, self.raw) }
    }

    /// ToNumber (may run user `valueOf` and throw).
    pub fn to_number(&self) -> JsResult<f64> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let n = unsafe { sys::JSValueToNumber(self.ctx, self.raw, &mut exc) };
        check(self.ctx, exc).map(|_| n)
    }

    /// ToString (may run user `toString` and throw).
    pub fn to_js_string(&self) -> JsResult<JsString> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let s = unsafe { sys::JSValueToStringCopy(self.ctx, self.raw, &mut exc) };
        check(self.ctx, exc)?;
        // SAFETY: non-null on success, returned +1.
        Ok(unsafe { JsString::from_raw_owned(s) })
    }

    /// ToString converted to UTF-8.
    pub fn to_string(&self) -> JsResult<String> {
        // A string's own characters, copied once (patched JSC): no
        // `JSStringRef` copy, no UTF-16 widening of an 8-bit string. While a
        // termination propagates, `check` below reports it, as every call does.
        if !crate::engine::callback_tls().terminating.get() {
            if let Some(units) = self.string_units_anywhere() {
                return Ok(units.to_string_lossy());
            }
        }
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let s = unsafe { sys::JSValueToStringCopy(self.ctx, self.raw, &mut exc) };
        check(self.ctx, exc)?;
        // SAFETY: as above; released right after conversion.
        unsafe {
            let out = to_rust_string(s);
            sys::JSStringRelease(s);
            Ok(out)
        }
    }

    /// If this is a string, its UTF-8 contents; no user code runs.
    pub fn as_string(&self) -> Option<String> {
        if sys::spi::direct_strings().is_some() && !crate::engine::callback_tls().terminating.get() {
            return self.string_units_anywhere().map(|u| u.to_string_lossy());
        }
        if self.is_string() {
            self.to_string().ok()
        } else {
            None
        }
    }

    /// `JSON.stringify(value)`; `Ok(None)` when the result is `undefined`.
    pub fn to_json(&self, indent: u32) -> JsResult<Option<String>> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let s = unsafe { sys::JSValueCreateJSONString(self.ctx, self.raw, indent, &mut exc) };
        check(self.ctx, exc)?;
        if s.is_null() {
            return Ok(None);
        }
        unsafe {
            let out = to_rust_string(s);
            sys::JSStringRelease(s);
            Ok(Some(out))
        }
    }

    pub fn strict_equals(&self, other: Value<'_>) -> bool {
        unsafe { sys::JSValueIsStrictEqual(self.ctx, self.raw, other.raw) }
    }

    pub fn instance_of(&self, constructor: Object<'_>) -> JsResult<bool> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let r = unsafe { sys::JSValueIsInstanceOfConstructor(self.ctx, self.raw, constructor.raw, &mut exc) };
        check(self.ctx, exc).map(|_| r)
    }
}

impl<'s> Object<'s> {
    /// # Safety
    /// Same contract as [`Value::from_raw`].
    pub unsafe fn from_raw(ctx: sys::JSContextRef, raw: sys::JSObjectRef) -> Object<'s> {
        debug_assert!(!raw.is_null());
        Object { raw, ctx, _scope: PhantomData }
    }

    pub fn as_raw(&self) -> sys::JSObjectRef {
        self.raw
    }

    pub fn value(&self) -> Value<'s> {
        Value { raw: self.raw as sys::JSValueRef, ctx: self.ctx, _scope: PhantomData }
    }

    pub fn is_function(&self) -> bool {
        unsafe { sys::JSObjectIsFunction(self.ctx, self.raw) }
    }

    pub fn is_constructor(&self) -> bool {
        unsafe { sys::JSObjectIsConstructor(self.ctx, self.raw) }
    }

    pub fn get(&self, name: &str) -> JsResult<Value<'s>> {
        self.get_js(&JsString::new(name))
    }

    pub fn get_js(&self, name: &JsString) -> JsResult<Value<'s>> {
        self.get_raw_name(name.as_raw())
    }

    /// [`Object::get`] for a constant name: the key is the thread's string
    /// for `name` ([`crate::atom`]), made once instead of per call.
    pub fn get_atom(&self, name: &'static str) -> JsResult<Value<'s>> {
        self.get_raw_name(crate::string::atom_raw(name))
    }

    fn get_raw_name(&self, name: sys::JSStringRef) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let v = unsafe { sys::JSObjectGetProperty(self.ctx, self.raw, name, &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    pub fn get_index(&self, index: u32) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let v = unsafe { sys::JSObjectGetPropertyAtIndex(self.ctx, self.raw, index, &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    /// `obj[key]` for an arbitrary key value (string, symbol, number).
    pub fn get_key(&self, key: Value<'_>) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let v = unsafe { sys::JSObjectGetPropertyForKey(self.ctx, self.raw, key.raw, &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    pub fn set(&self, name: &str, value: Value<'_>) -> JsResult<()> {
        self.set_with(&JsString::new(name), value, sys::kJSPropertyAttributeNone)
    }

    pub fn set_with(&self, name: &JsString, value: Value<'_>, attrs: sys::JSPropertyAttributes) -> JsResult<()> {
        self.set_raw_name(name.as_raw(), value, attrs)
    }

    /// [`Object::set`] for a constant name (see [`Object::get_atom`]).
    pub fn set_atom(&self, name: &'static str, value: Value<'_>) -> JsResult<()> {
        self.set_raw_name(crate::string::atom_raw(name), value, sys::kJSPropertyAttributeNone)
    }

    /// [`Object::set_with`] for a constant name (see [`Object::get_atom`]).
    pub fn set_atom_with(
        &self,
        name: &'static str,
        value: Value<'_>,
        attrs: sys::JSPropertyAttributes,
    ) -> JsResult<()> {
        self.set_raw_name(crate::string::atom_raw(name), value, attrs)
    }

    fn set_raw_name(&self, name: sys::JSStringRef, value: Value<'_>, attrs: sys::JSPropertyAttributes) -> JsResult<()> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        unsafe { sys::JSObjectSetProperty(self.ctx, self.raw, name, value.raw, attrs, &mut exc) };
        check(self.ctx, exc)
    }

    pub fn set_index(&self, index: u32, value: Value<'_>) -> JsResult<()> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        unsafe { sys::JSObjectSetPropertyAtIndex(self.ctx, self.raw, index, value.raw, &mut exc) };
        check(self.ctx, exc)
    }

    pub fn has(&self, name: &str) -> bool {
        let _pool = crate::autorelease::Pool::new();
        unsafe { sys::JSObjectHasProperty(self.ctx, self.raw, JsString::new(name).as_raw()) }
    }

    /// [`Object::has`] for a constant name (see [`Object::get_atom`]).
    pub fn has_atom(&self, name: &'static str) -> bool {
        let _pool = crate::autorelease::Pool::new();
        unsafe { sys::JSObjectHasProperty(self.ctx, self.raw, crate::string::atom_raw(name)) }
    }

    pub fn delete(&self, name: &str) -> JsResult<bool> {
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let r = unsafe { sys::JSObjectDeleteProperty(self.ctx, self.raw, JsString::new(name).as_raw(), &mut exc) };
        check(self.ctx, exc).map(|_| r)
    }

    pub fn prototype(&self) -> Value<'s> {
        let _pool = crate::autorelease::Pool::new();
        unsafe { Value::from_raw(self.ctx, sys::JSObjectGetPrototype(self.ctx, self.raw)) }
    }

    pub fn set_prototype(&self, proto: Value<'_>) {
        let _pool = crate::autorelease::Pool::new();
        unsafe { sys::JSObjectSetPrototype(self.ctx, self.raw, proto.raw) }
    }

    /// Own enumerable string-keyed property names (`Object.keys` order).
    pub fn keys(&self) -> Vec<String> {
        let _pool = crate::autorelease::Pool::new();
        unsafe {
            let names = sys::JSObjectCopyPropertyNames(self.ctx, self.raw);
            let n = sys::JSPropertyNameArrayGetCount(names);
            let out = (0..n).map(|i| to_rust_string(sys::JSPropertyNameArrayGetNameAtIndex(names, i))).collect();
            sys::JSPropertyNameArrayRelease(names);
            out
        }
    }

    /// Calls this object as a function. `args` must be rooted by the caller
    /// (stack locals, or use [`Object::call_rooted`] for values collected on
    /// the heap); JSC roots them for the duration of the call.
    pub fn call(&self, this: Option<Object<'_>>, args: &[Value<'_>]) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let raw_args = RawArgs::new(args);
        let mut exc = ptr::null();
        let this_raw = this.map_or(ptr::null_mut(), |o| o.raw);
        let v = unsafe {
            sys::JSObjectCallAsFunction(self.ctx, self.raw, this_raw, raw_args.len(), raw_args.as_ptr(), &mut exc)
        };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    /// Calls with an arbitrary `this` value (primitive `this` is boxed by JSC
    /// rules for sloppy functions; strict functions see `undefined` for non-objects).
    pub fn call_with_this_value(&self, this: Value<'_>, args: &[Value<'_>]) -> JsResult<Value<'s>> {
        self.call(this.as_object(), args)
    }

    /// Calls with arguments held in a [`crate::RootedVec`]: the safe way to pass
    /// many arguments that were produced one by one (each conversion may GC).
    pub fn call_rooted(&self, this: Option<Object<'_>>, args: &crate::RootedVec) -> JsResult<Value<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let raw = args.raw();
        let mut exc = ptr::null();
        let this_raw = this.map_or(ptr::null_mut(), |o| o.raw);
        let v = unsafe { sys::JSObjectCallAsFunction(self.ctx, self.raw, this_raw, raw.len(), raw.as_ptr(), &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Value::from_raw(self.ctx, v) })
    }

    pub fn construct(&self, args: &[Value<'_>]) -> JsResult<Object<'s>> {
        let _pool = crate::autorelease::Pool::new();
        let raw_args = RawArgs::new(args);
        let mut exc = ptr::null();
        let o =
            unsafe { sys::JSObjectCallAsConstructor(self.ctx, self.raw, raw_args.len(), raw_args.as_ptr(), &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Object::from_raw(self.ctx, o) })
    }
}

/// Argument buffer: inline on the stack for up to 8 values so the raw refs stay
/// visible to the conservative scanner without heap allocation.
struct RawArgs {
    inline: [sys::JSValueRef; 8],
    heap: Vec<sys::JSValueRef>,
    len: usize,
}

impl RawArgs {
    fn new(args: &[Value<'_>]) -> RawArgs {
        let mut r = RawArgs { inline: [ptr::null(); 8], heap: Vec::new(), len: args.len() };
        if args.len() <= 8 {
            for (slot, a) in r.inline.iter_mut().zip(args) {
                *slot = a.raw;
            }
        } else {
            // The originals in `args` remain the GC roots; this copy only feeds
            // the call, which roots them again before allocating.
            r.heap = args.iter().map(|a| a.raw).collect();
        }
        r
    }
    fn len(&self) -> usize {
        self.len
    }
    fn as_ptr(&self) -> *const sys::JSValueRef {
        if self.len <= 8 {
            self.inline.as_ptr()
        } else {
            self.heap.as_ptr()
        }
    }
}

/// Converts a JSC exception out-parameter into a `JsError`. While a
/// termination is propagating every call reports `Terminated` (even if page
/// code caught the termination value); back at depth 0 the termination state
/// is reset.
pub(crate) fn check(ctx: sys::JSContextRef, exc: sys::JSValueRef) -> JsResult<()> {
    let tls = crate::engine::callback_tls();
    if tls.terminating.get() {
        if tls.depth.get() == 0 {
            crate::engine::finish_termination(ctx);
        }
        return Err(JsError::Terminated);
    }
    if exc.is_null() {
        Ok(())
    } else {
        Err(JsError::thrown_raw(ctx, exc))
    }
}

/// A JavaScript string's characters as JSC keeps them ([`Value::string_units`]).
#[derive(Clone, Copy, Debug)]
pub enum StringUnits<'a> {
    Latin1(&'a [u8]),
    Utf16(&'a [u16]),
}

impl StringUnits<'_> {
    /// FxHash of the code units, the same for either representation.
    pub fn fx_hash(&self) -> u64 {
        use std::hash::Hasher;
        let mut h = rustc_hash::FxHasher::default();
        match self {
            StringUnits::Latin1(b) => b.iter().for_each(|&c| h.write_u16(u16::from(c))),
            StringUnits::Utf16(u) => u.iter().for_each(|&c| h.write_u16(c)),
        }
        h.finish()
    }

    pub fn eq_units(&self, units: &[u16]) -> bool {
        match self {
            StringUnits::Latin1(b) => b.len() == units.len() && b.iter().zip(units).all(|(&c, &u)| u16::from(c) == u),
            StringUnits::Utf16(u) => *u == units,
        }
    }

    pub fn to_units(&self) -> Box<[u16]> {
        match self {
            StringUnits::Latin1(b) => b.iter().map(|&c| u16::from(c)).collect(),
            StringUnits::Utf16(u) => (*u).into(),
        }
    }

    pub fn to_string_lossy(&self) -> String {
        match self {
            StringUnits::Latin1(b) => latin1_to_string(b),
            StringUnits::Utf16(u) => utf16_to_string(u),
        }
    }
}

/// Arguments of a native callback.
#[derive(Clone, Copy)]
pub struct Args<'s> {
    pub(crate) ctx: sys::JSContextRef,
    pub(crate) raw: &'s [sys::JSValueRef],
}

impl<'s> Args<'s> {
    /// # Safety
    /// `argv`/`argc` must describe JSC's argument array for the current call.
    pub(crate) unsafe fn from_raw(ctx: sys::JSContextRef, argc: usize, argv: *const sys::JSValueRef) -> Args<'s> {
        let raw = if argc == 0 || argv.is_null() { &[][..] } else { unsafe { std::slice::from_raw_parts(argv, argc) } };
        Args { ctx, raw }
    }

    pub fn len(&self) -> usize {
        self.raw.len()
    }

    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Argument `i`, or `undefined` when absent.
    pub fn get(&self, i: usize) -> Value<'s> {
        match self.raw.get(i) {
            Some(v) => unsafe { Value::from_raw(self.ctx, *v) },
            None => unsafe { Value::from_raw(self.ctx, sys::JSValueMakeUndefined(self.ctx)) },
        }
    }

    /// Argument `i` if present (distinguishes missing from explicit undefined).
    pub fn opt(&self, i: usize) -> Option<Value<'s>> {
        self.raw.get(i).map(|v| unsafe { Value::from_raw(self.ctx, *v) })
    }

    /// Fails with a WebIDL-style TypeError when fewer than `n` arguments.
    pub fn require(&self, n: usize, what: &str) -> JsResult<()> {
        if self.raw.len() < n {
            return Err(JsError::Type(format!(
                "Failed to execute '{what}': {n} argument{} required, but only {} present.",
                if n == 1 { "" } else { "s" },
                self.raw.len()
            )));
        }
        Ok(())
    }

    pub fn iter(&self) -> impl Iterator<Item = Value<'s>> + '_ {
        (0..self.len()).map(move |i| self.get(i))
    }
}

// JavaScriptCore's 64-bit immediates (JSCJSValue.h). Getters return these
// tens of thousands of times per selector query, and each `JSValueMake*` is
// a call into the library; `verify_immediates` checks them against it once
// per process, and they are used only if every one matches.
const IMM_NULL: usize = 0x2;
const IMM_FALSE: usize = 0x6;
const IMM_TRUE: usize = 0x7;
const IMM_UNDEFINED: usize = 0xa;
const IMM_INT32_TAG: usize = 0xfffe_0000_0000_0000;

/// 0: not checked; 1: the immediates match this JavaScriptCore; 2: they do not.
static IMMEDIATES: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

#[inline]
fn immediates_verified() -> bool {
    IMMEDIATES.load(std::sync::atomic::Ordering::Relaxed) == 1
}

/// Compares the immediates above with what JavaScriptCore makes (once per
/// process, from engine creation).
pub(crate) fn verify_immediates(ctx: sys::JSContextRef) {
    use std::sync::atomic::Ordering;
    if IMMEDIATES.load(Ordering::Relaxed) != 0 || !cfg!(target_pointer_width = "64") {
        return;
    }
    // SAFETY: a live context; these calls make immediates.
    let ok = unsafe {
        sys::JSValueMakeNull(ctx) as usize == IMM_NULL
            && sys::JSValueMakeUndefined(ctx) as usize == IMM_UNDEFINED
            && sys::JSValueMakeBoolean(ctx, true) as usize == IMM_TRUE
            && sys::JSValueMakeBoolean(ctx, false) as usize == IMM_FALSE
            && [0i32, 1, -1, 2000, i32::MAX, i32::MIN]
                .iter()
                .all(|&i| sys::JSValueMakeNumber(ctx, f64::from(i)) as usize == IMM_INT32_TAG | i as u32 as usize)
    };
    IMMEDIATES.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
}

/// Value constructors bound to a scope.
impl<'s> Scope<'s> {
    pub fn undefined(&self) -> Value<'s> {
        if immediates_verified() {
            return unsafe { Value::from_raw(self.ctx, IMM_UNDEFINED as sys::JSValueRef) };
        }
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeUndefined(self.ctx)) }
    }
    pub fn null(&self) -> Value<'s> {
        if immediates_verified() {
            return unsafe { Value::from_raw(self.ctx, IMM_NULL as sys::JSValueRef) };
        }
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeNull(self.ctx)) }
    }
    pub fn boolean(&self, b: bool) -> Value<'s> {
        if immediates_verified() {
            return unsafe { Value::from_raw(self.ctx, (if b { IMM_TRUE } else { IMM_FALSE }) as sys::JSValueRef) };
        }
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeBoolean(self.ctx, b)) }
    }
    pub fn number(&self, n: f64) -> Value<'s> {
        // An int32 (not negative zero) is an immediate: no call into JSC.
        let i = n as i32;
        if f64::from(i) == n && (i != 0 || n.is_sign_positive()) && immediates_verified() {
            return unsafe { Value::from_raw(self.ctx, (IMM_INT32_TAG | i as u32 as usize) as sys::JSValueRef) };
        }
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeNumber(self.ctx, n)) }
    }
    pub fn string(&self, s: &str) -> Value<'s> {
        // Patched JSC makes the value from the characters: one copy into the
        // string, where `JSValueMakeString` needs a `JSStringRef` made and
        // released around it, copies twice and takes the API lock.
        if let Some(d) = sys::spi::direct_strings() {
            // SAFETY: a live context of this thread; JSC copies the slice.
            let v = with_host_units(s, |units| unsafe {
                match units {
                    HostUnits::Latin1(b) => (d.make_latin1)(self.ctx, b.as_ptr(), b.len()),
                    HostUnits::Utf16(u) => (d.make_utf16)(self.ctx, u.as_ptr(), u.len()),
                }
            });
            if !v.is_null() {
                return unsafe { Value::from_raw(self.ctx, v) };
            }
        }
        self.js_string(&JsString::new(s))
    }
    /// A string of UTF-16 code units, unpaired surrogates kept (what a
    /// `DOMString` may hold and `&str` cannot).
    pub fn string_utf16(&self, units: &[u16]) -> Value<'s> {
        if let Some(d) = sys::spi::direct_strings() {
            // SAFETY: as in `string`.
            let v = unsafe { (d.make_utf16)(self.ctx, units.as_ptr(), units.len()) };
            if !v.is_null() {
                return unsafe { Value::from_raw(self.ctx, v) };
            }
        }
        self.js_string(&JsString::from_utf16(units))
    }
    /// The realm's one JavaScript string for a shared host string (see
    /// `RealmInner::shared_strings`): for values with few distinct contents
    /// that the host keeps alive, such as tag names.
    pub fn shared_string(&self, s: &std::sync::Arc<str>) -> Value<'s> {
        let key = std::sync::Arc::as_ptr(s) as *const u8 as usize;
        if let Some((_, v)) = self.realm.shared_strings.borrow().get(&key) {
            return unsafe { Value::from_raw(self.ctx, *v) };
        }
        let v = self.string(s);
        // SAFETY: a live value of this context; released in `dispose`.
        unsafe { sys::JSValueProtect(self.ctx, v.raw) };
        self.realm.shared_strings.borrow_mut().insert(key, (s.clone(), v.raw));
        v
    }
    pub fn js_string(&self, s: &JsString) -> Value<'s> {
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeString(self.ctx, s.as_raw())) }
    }
    pub fn object(&self) -> Object<'s> {
        unsafe { Object::from_raw(self.ctx, sys::JSObjectMake(self.ctx, ptr::null(), ptr::null_mut())) }
    }
    pub fn array(&self, items: &[Value<'_>]) -> JsResult<Object<'s>> {
        let raw = RawArgs::new(items);
        let mut exc = ptr::null();
        let o = unsafe { sys::JSObjectMakeArray(self.ctx, raw.len(), raw.as_ptr(), &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Object::from_raw(self.ctx, o) })
    }
    /// `new Date(time)` with the realm's intrinsic `Date`.
    pub fn date(&self, time: f64) -> JsResult<Object<'s>> {
        let arg = self.number(time);
        let raw = RawArgs::new(std::slice::from_ref(&arg));
        let mut exc = ptr::null();
        let o = unsafe { sys::JSObjectMakeDate(self.ctx, raw.len(), raw.as_ptr(), &mut exc) };
        check(self.ctx, exc)?;
        Ok(unsafe { Object::from_raw(self.ctx, o) })
    }
    /// `JSON.parse`; `None` when `json` is not valid JSON.
    pub fn parse_json(&self, json: &str) -> Option<Value<'s>> {
        let v = unsafe { sys::JSValueMakeFromJSONString(self.ctx, JsString::new(json).as_raw()) };
        (!v.is_null()).then(|| unsafe { Value::from_raw(self.ctx, v) })
    }
    pub fn global(&self) -> Object<'s> {
        unsafe { Object::from_raw(self.ctx, self.global_object_raw()) }
    }

    /// `value`, a value of another realm of this engine, as a value of this
    /// scope: the same JavaScript value, used with this realm's context from
    /// now on (no copy, no protect/unprotect, unlike a round trip through a
    /// [`crate::Persistent`]).
    ///
    /// Sound because a JavaScript value belongs to the VM, not to a realm:
    /// every context of the engine's context group can use it, and the
    /// result's context is this scope's, alive for `'s`. Rooting is
    /// unchanged: like any [`Value`], the result is kept alive by being on
    /// the stack (see the module docs), not by the realm it came from, so a
    /// value that was only reachable from a scope that has since ended must
    /// still be held in a local. Panics for a value of another engine,
    /// which no context of this one may touch.
    pub fn rehome(&self, value: Value<'_>) -> Value<'s> {
        self.assert_same_engine(value.ctx);
        // SAFETY: a live value of this VM (checked above), rooted per the module rule.
        unsafe { Value::from_raw(self.ctx, value.raw) }
    }

    /// [`Scope::rehome`] for an object.
    pub fn rehome_object(&self, object: Object<'_>) -> Object<'s> {
        self.assert_same_engine(object.ctx);
        // SAFETY: as in `rehome`.
        unsafe { Object::from_raw(self.ctx, object.raw) }
    }

    fn assert_same_engine(&self, ctx: sys::JSContextRef) {
        // SAFETY: both contexts are live (the value's scope, or another
        // realm of a live engine); reading the group runs no script.
        assert!(
            ctx == self.ctx || unsafe { sys::JSContextGetGroup(ctx) } == self.engine_inner().group(),
            "rehome: the value belongs to another engine"
        );
    }
}

impl std::fmt::Debug for Value<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Value({:?}, {:p})", self.ty(), self.raw)
    }
}

impl std::fmt::Debug for Object<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Object({:p})", self.raw)
    }
}
