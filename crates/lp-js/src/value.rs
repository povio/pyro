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
use crate::string::{to_rust_string, JsString};
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
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        let v = unsafe { sys::JSObjectGetProperty(self.ctx, self.raw, name.as_raw(), &mut exc) };
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
        let _pool = crate::autorelease::Pool::new();
        let mut exc = ptr::null();
        unsafe { sys::JSObjectSetProperty(self.ctx, self.raw, name.as_raw(), value.raw, attrs, &mut exc) };
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
    if crate::engine::TERMINATING.with(|t| t.get()) {
        if crate::engine::DEPTH.with(|d| d.get()) == 0 {
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

/// Value constructors bound to a scope.
impl<'s> Scope<'s> {
    pub fn undefined(&self) -> Value<'s> {
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeUndefined(self.ctx)) }
    }
    pub fn null(&self) -> Value<'s> {
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeNull(self.ctx)) }
    }
    pub fn boolean(&self, b: bool) -> Value<'s> {
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeBoolean(self.ctx, b)) }
    }
    pub fn number(&self, n: f64) -> Value<'s> {
        unsafe { Value::from_raw(self.ctx, sys::JSValueMakeNumber(self.ctx, n)) }
    }
    pub fn string(&self, s: &str) -> Value<'s> {
        self.js_string(&JsString::new(s))
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
    /// `JSON.parse`; `None` when `json` is not valid JSON.
    pub fn parse_json(&self, json: &str) -> Option<Value<'s>> {
        let v = unsafe { sys::JSValueMakeFromJSONString(self.ctx, JsString::new(json).as_raw()) };
        (!v.is_null()).then(|| unsafe { Value::from_raw(self.ctx, v) })
    }
    pub fn global(&self) -> Object<'s> {
        unsafe { Object::from_raw(self.ctx, sys::JSContextGetGlobalObject(self.ctx)) }
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
