//! Raw bindings to the JavaScriptCore C API.
//!
//! Hand-written (no bindgen/libclang build dependency) from the macOS SDK
//! headers `JSBase.h`, `JSContextRef.h`, `JSStringRef.h`, `JSValueRef.h`,
//! `JSObjectRef.h` and `JSTypedArray.h`. The `spi` module declares functions
//! that are exported by the system framework but only declared in WebKit's
//! private headers; their availability is probed by tests in this crate.
//!
//! Everything here is `unsafe`; the safe layer lives in `lp-js`.
#![allow(non_camel_case_types, non_upper_case_globals, non_snake_case)]

use std::os::raw::{c_char, c_double, c_int, c_uint, c_void};

macro_rules! opaque {
    ($($name:ident => $ref:ident),* $(,)?) => {$(
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
            _pin: core::marker::PhantomData<(*mut u8, core::marker::PhantomPinned)>,
        }
        pub type $ref = *const $name;
    )*};
}

opaque! {
    OpaqueJSContextGroup => JSContextGroupRef,
    OpaqueJSContext => JSContextRef,
    OpaqueJSString => JSStringRef,
    OpaqueJSClass => JSClassRef,
    OpaqueJSPropertyNameArray => JSPropertyNameArrayRef,
    OpaqueJSPropertyNameAccumulator => JSPropertyNameAccumulatorRef,
    OpaqueJSValue => JSValueRef,
    OpaqueJSWeak => JSWeakRef,
    OpaqueJSWeakObjectMap => JSWeakObjectMapRef,
}

pub type JSGlobalContextRef = *mut OpaqueJSContext;
pub type JSObjectRef = *mut OpaqueJSValue;
pub type JSChar = u16;

pub type JSType = c_uint;
pub const kJSTypeUndefined: JSType = 0;
pub const kJSTypeNull: JSType = 1;
pub const kJSTypeBoolean: JSType = 2;
pub const kJSTypeNumber: JSType = 3;
pub const kJSTypeString: JSType = 4;
pub const kJSTypeObject: JSType = 5;
pub const kJSTypeSymbol: JSType = 6;
pub const kJSTypeBigInt: JSType = 7;

pub type JSTypedArrayType = c_uint;
pub const kJSTypedArrayTypeInt8Array: JSTypedArrayType = 0;
pub const kJSTypedArrayTypeInt16Array: JSTypedArrayType = 1;
pub const kJSTypedArrayTypeInt32Array: JSTypedArrayType = 2;
pub const kJSTypedArrayTypeUint8Array: JSTypedArrayType = 3;
pub const kJSTypedArrayTypeUint8ClampedArray: JSTypedArrayType = 4;
pub const kJSTypedArrayTypeUint16Array: JSTypedArrayType = 5;
pub const kJSTypedArrayTypeUint32Array: JSTypedArrayType = 6;
pub const kJSTypedArrayTypeFloat32Array: JSTypedArrayType = 7;
pub const kJSTypedArrayTypeFloat64Array: JSTypedArrayType = 8;
pub const kJSTypedArrayTypeArrayBuffer: JSTypedArrayType = 9;
pub const kJSTypedArrayTypeNone: JSTypedArrayType = 10;
pub const kJSTypedArrayTypeBigInt64Array: JSTypedArrayType = 11;
pub const kJSTypedArrayTypeBigUint64Array: JSTypedArrayType = 12;

pub type JSPropertyAttributes = c_uint;
pub const kJSPropertyAttributeNone: JSPropertyAttributes = 0;
pub const kJSPropertyAttributeReadOnly: JSPropertyAttributes = 1 << 1;
pub const kJSPropertyAttributeDontEnum: JSPropertyAttributes = 1 << 2;
pub const kJSPropertyAttributeDontDelete: JSPropertyAttributes = 1 << 3;

pub type JSClassAttributes = c_uint;
pub const kJSClassAttributeNone: JSClassAttributes = 0;
pub const kJSClassAttributeNoAutomaticPrototype: JSClassAttributes = 1 << 1;
/// Patched JavaScriptCore only (`webkit/patches/0015-impure-named-properties.patch`): the
/// class's hooks answer only names announced with `JSAddImpureProperty`, so its wrappers'
/// lookups stay cacheable. Stock JavaScriptCore ignores the bit.
pub const kJSClassAttributeImpureNamedProperties: JSClassAttributes = 1 << 30;
/// Patched JSC only (`JSForkPrivate.h`; the system framework ignores the
/// bit): the class is never released, so its objects count no reference to
/// it (one atomic word every thread making or sweeping its wrappers writes).
pub const kJSClassAttributeImmortal: JSClassAttributes = 1 << 29;

pub type JSRelationCondition = c_uint;

pub type JSObjectInitializeCallback = Option<unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef)>;
pub type JSObjectFinalizeCallback = Option<unsafe extern "C" fn(object: JSObjectRef)>;
pub type JSObjectHasPropertyCallback =
    Option<unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef) -> bool>;
pub type JSObjectGetPropertyCallback = Option<
    unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef,
>;
pub type JSObjectSetPropertyCallback = Option<
    unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> bool,
>;
pub type JSObjectDeletePropertyCallback = Option<
    unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef, exception: *mut JSValueRef) -> bool,
>;
pub type JSObjectGetPropertyNamesCallback =
    Option<unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, names: JSPropertyNameAccumulatorRef)>;
pub type JSObjectCallAsFunctionCallbackRaw = unsafe extern "C" fn(
    ctx: JSContextRef,
    function: JSObjectRef,
    this: JSObjectRef,
    argc: usize,
    argv: *const JSValueRef,
    exception: *mut JSValueRef,
) -> JSValueRef;
pub type JSObjectCallAsFunctionCallback = Option<JSObjectCallAsFunctionCallbackRaw>;
pub type JSObjectCallAsConstructorCallback = Option<
    unsafe extern "C" fn(
        ctx: JSContextRef,
        constructor: JSObjectRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef,
>;
pub type JSObjectHasInstanceCallback = Option<
    unsafe extern "C" fn(
        ctx: JSContextRef,
        constructor: JSObjectRef,
        instance: JSValueRef,
        exception: *mut JSValueRef,
    ) -> bool,
>;
pub type JSObjectConvertToTypeCallback = Option<
    unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, ty: JSType, exception: *mut JSValueRef) -> JSValueRef,
>;
pub type JSTypedArrayBytesDeallocator = Option<unsafe extern "C" fn(bytes: *mut c_void, context: *mut c_void)>;

#[repr(C)]
pub struct JSStaticValue {
    pub name: *const c_char,
    pub getProperty: JSObjectGetPropertyCallback,
    pub setProperty: JSObjectSetPropertyCallback,
    pub attributes: JSPropertyAttributes,
}

#[repr(C)]
pub struct JSStaticFunction {
    pub name: *const c_char,
    pub callAsFunction: JSObjectCallAsFunctionCallback,
    pub attributes: JSPropertyAttributes,
}

#[repr(C)]
pub struct JSClassDefinition {
    pub version: c_int,
    pub attributes: JSClassAttributes,
    pub className: *const c_char,
    pub parentClass: JSClassRef,
    pub staticValues: *const JSStaticValue,
    pub staticFunctions: *const JSStaticFunction,
    pub initialize: JSObjectInitializeCallback,
    pub finalize: JSObjectFinalizeCallback,
    pub hasProperty: JSObjectHasPropertyCallback,
    pub getProperty: JSObjectGetPropertyCallback,
    pub setProperty: JSObjectSetPropertyCallback,
    pub deleteProperty: JSObjectDeletePropertyCallback,
    pub getPropertyNames: JSObjectGetPropertyNamesCallback,
    pub callAsFunction: JSObjectCallAsFunctionCallback,
    pub callAsConstructor: JSObjectCallAsConstructorCallback,
    pub hasInstance: JSObjectHasInstanceCallback,
    pub convertToType: JSObjectConvertToTypeCallback,
}

impl JSClassDefinition {
    /// Equivalent of `kJSClassDefinitionEmpty`.
    pub const EMPTY: JSClassDefinition = JSClassDefinition {
        version: 0,
        attributes: kJSClassAttributeNone,
        className: core::ptr::null(),
        parentClass: core::ptr::null(),
        staticValues: core::ptr::null(),
        staticFunctions: core::ptr::null(),
        initialize: None,
        finalize: None,
        hasProperty: None,
        getProperty: None,
        setProperty: None,
        deleteProperty: None,
        getPropertyNames: None,
        callAsFunction: None,
        callAsConstructor: None,
        hasInstance: None,
        convertToType: None,
    };
}

unsafe extern "C" {
    // JSBase.h
    pub fn JSEvaluateScript(
        ctx: JSContextRef,
        script: JSStringRef,
        this: JSObjectRef,
        source_url: JSStringRef,
        starting_line: c_int,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSCheckScriptSyntax(
        ctx: JSContextRef,
        script: JSStringRef,
        source_url: JSStringRef,
        starting_line: c_int,
        exception: *mut JSValueRef,
    ) -> bool;
    pub fn JSGarbageCollect(ctx: JSContextRef);

    // JSContextRef.h
    pub fn JSContextGroupCreate() -> JSContextGroupRef;
    pub fn JSContextGroupRetain(group: JSContextGroupRef) -> JSContextGroupRef;
    pub fn JSContextGroupRelease(group: JSContextGroupRef);
    pub fn JSGlobalContextCreate(global_class: JSClassRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextCreateInGroup(group: JSContextGroupRef, global_class: JSClassRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextRetain(ctx: JSGlobalContextRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextRelease(ctx: JSGlobalContextRef);
    pub fn JSContextGetGlobalObject(ctx: JSContextRef) -> JSObjectRef;
    pub fn JSContextGetGroup(ctx: JSContextRef) -> JSContextGroupRef;
    pub fn JSContextGetGlobalContext(ctx: JSContextRef) -> JSGlobalContextRef;
    pub fn JSGlobalContextCopyName(ctx: JSGlobalContextRef) -> JSStringRef;
    pub fn JSGlobalContextSetName(ctx: JSGlobalContextRef, name: JSStringRef);
    pub fn JSGlobalContextIsInspectable(ctx: JSGlobalContextRef) -> bool;
    pub fn JSGlobalContextSetInspectable(ctx: JSGlobalContextRef, inspectable: bool);

    // JSStringRef.h
    pub fn JSStringCreateWithCharacters(chars: *const JSChar, num_chars: usize) -> JSStringRef;
    pub fn JSStringCreateWithUTF8CString(string: *const c_char) -> JSStringRef;
    pub fn JSStringRetain(string: JSStringRef) -> JSStringRef;
    pub fn JSStringRelease(string: JSStringRef);
    pub fn JSStringGetLength(string: JSStringRef) -> usize;
    pub fn JSStringGetCharactersPtr(string: JSStringRef) -> *const JSChar;
    pub fn JSStringGetMaximumUTF8CStringSize(string: JSStringRef) -> usize;
    pub fn JSStringGetUTF8CString(string: JSStringRef, buffer: *mut c_char, buffer_size: usize) -> usize;
    pub fn JSStringIsEqual(a: JSStringRef, b: JSStringRef) -> bool;
    pub fn JSStringIsEqualToUTF8CString(a: JSStringRef, b: *const c_char) -> bool;

    // JSValueRef.h
    pub fn JSValueGetType(ctx: JSContextRef, value: JSValueRef) -> JSType;
    pub fn JSValueIsUndefined(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsNull(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsBoolean(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsNumber(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsString(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsSymbol(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsBigInt(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsObject(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsObjectOfClass(ctx: JSContextRef, value: JSValueRef, class: JSClassRef) -> bool;
    pub fn JSValueIsArray(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueIsDate(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueGetTypedArrayType(
        ctx: JSContextRef,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSTypedArrayType;
    pub fn JSValueIsEqual(ctx: JSContextRef, a: JSValueRef, b: JSValueRef, exception: *mut JSValueRef) -> bool;
    pub fn JSValueIsStrictEqual(ctx: JSContextRef, a: JSValueRef, b: JSValueRef) -> bool;
    pub fn JSValueIsInstanceOfConstructor(
        ctx: JSContextRef,
        value: JSValueRef,
        constructor: JSObjectRef,
        exception: *mut JSValueRef,
    ) -> bool;
    pub fn JSValueMakeUndefined(ctx: JSContextRef) -> JSValueRef;
    pub fn JSValueMakeNull(ctx: JSContextRef) -> JSValueRef;
    pub fn JSValueMakeBoolean(ctx: JSContextRef, boolean: bool) -> JSValueRef;
    pub fn JSValueMakeNumber(ctx: JSContextRef, number: c_double) -> JSValueRef;
    pub fn JSValueMakeString(ctx: JSContextRef, string: JSStringRef) -> JSValueRef;
    pub fn JSValueMakeSymbol(ctx: JSContextRef, description: JSStringRef) -> JSValueRef;
    pub fn JSValueMakeFromJSONString(ctx: JSContextRef, string: JSStringRef) -> JSValueRef;
    pub fn JSValueCreateJSONString(
        ctx: JSContextRef,
        value: JSValueRef,
        indent: c_uint,
        exception: *mut JSValueRef,
    ) -> JSStringRef;
    pub fn JSValueToBoolean(ctx: JSContextRef, value: JSValueRef) -> bool;
    pub fn JSValueToNumber(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> c_double;
    pub fn JSValueToInt32(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> i32;
    pub fn JSValueToUInt32(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> u32;
    pub fn JSValueToStringCopy(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> JSStringRef;
    pub fn JSValueToObject(ctx: JSContextRef, value: JSValueRef, exception: *mut JSValueRef) -> JSObjectRef;
    pub fn JSValueProtect(ctx: JSContextRef, value: JSValueRef);
    pub fn JSValueUnprotect(ctx: JSContextRef, value: JSValueRef);

    // JSObjectRef.h
    pub fn JSClassCreate(definition: *const JSClassDefinition) -> JSClassRef;
    pub fn JSClassRetain(class: JSClassRef) -> JSClassRef;
    pub fn JSClassRelease(class: JSClassRef);
    pub fn JSObjectMake(ctx: JSContextRef, class: JSClassRef, data: *mut c_void) -> JSObjectRef;
    pub fn JSObjectMakeFunctionWithCallback(
        ctx: JSContextRef,
        name: JSStringRef,
        call: JSObjectCallAsFunctionCallback,
    ) -> JSObjectRef;
    pub fn JSObjectMakeConstructor(
        ctx: JSContextRef,
        class: JSClassRef,
        call: JSObjectCallAsConstructorCallback,
    ) -> JSObjectRef;
    pub fn JSObjectMakeArray(
        ctx: JSContextRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeDate(
        ctx: JSContextRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeError(
        ctx: JSContextRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeRegExp(
        ctx: JSContextRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeDeferredPromise(
        ctx: JSContextRef,
        resolve: *mut JSObjectRef,
        reject: *mut JSObjectRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeFunction(
        ctx: JSContextRef,
        name: JSStringRef,
        param_count: c_uint,
        param_names: *const JSStringRef,
        body: JSStringRef,
        source_url: JSStringRef,
        starting_line: c_int,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectGetPrototype(ctx: JSContextRef, object: JSObjectRef) -> JSValueRef;
    pub fn JSObjectSetPrototype(ctx: JSContextRef, object: JSObjectRef, value: JSValueRef);
    pub fn JSObjectHasProperty(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef) -> bool;
    pub fn JSObjectGetProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectSetProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        value: JSValueRef,
        attributes: JSPropertyAttributes,
        exception: *mut JSValueRef,
    );
    pub fn JSObjectDeleteProperty(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        exception: *mut JSValueRef,
    ) -> bool;
    pub fn JSObjectHasPropertyForKey(
        ctx: JSContextRef,
        object: JSObjectRef,
        key: JSValueRef,
        exception: *mut JSValueRef,
    ) -> bool;
    pub fn JSObjectGetPropertyForKey(
        ctx: JSContextRef,
        object: JSObjectRef,
        key: JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectSetPropertyForKey(
        ctx: JSContextRef,
        object: JSObjectRef,
        key: JSValueRef,
        value: JSValueRef,
        attributes: JSPropertyAttributes,
        exception: *mut JSValueRef,
    );
    pub fn JSObjectDeletePropertyForKey(
        ctx: JSContextRef,
        object: JSObjectRef,
        key: JSValueRef,
        exception: *mut JSValueRef,
    ) -> bool;
    pub fn JSObjectGetPropertyAtIndex(
        ctx: JSContextRef,
        object: JSObjectRef,
        index: c_uint,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectSetPropertyAtIndex(
        ctx: JSContextRef,
        object: JSObjectRef,
        index: c_uint,
        value: JSValueRef,
        exception: *mut JSValueRef,
    );
    pub fn JSObjectGetPrivate(object: JSObjectRef) -> *mut c_void;
    pub fn JSObjectSetPrivate(object: JSObjectRef, data: *mut c_void) -> bool;
    pub fn JSObjectIsFunction(ctx: JSContextRef, object: JSObjectRef) -> bool;
    pub fn JSObjectCallAsFunction(
        ctx: JSContextRef,
        object: JSObjectRef,
        this: JSObjectRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub fn JSObjectIsConstructor(ctx: JSContextRef, object: JSObjectRef) -> bool;
    pub fn JSObjectCallAsConstructor(
        ctx: JSContextRef,
        object: JSObjectRef,
        argc: usize,
        argv: *const JSValueRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectCopyPropertyNames(ctx: JSContextRef, object: JSObjectRef) -> JSPropertyNameArrayRef;
    pub fn JSPropertyNameArrayRetain(array: JSPropertyNameArrayRef) -> JSPropertyNameArrayRef;
    pub fn JSPropertyNameArrayRelease(array: JSPropertyNameArrayRef);
    pub fn JSPropertyNameArrayGetCount(array: JSPropertyNameArrayRef) -> usize;
    pub fn JSPropertyNameArrayGetNameAtIndex(array: JSPropertyNameArrayRef, index: usize) -> JSStringRef;
    pub fn JSPropertyNameAccumulatorAddName(accumulator: JSPropertyNameAccumulatorRef, name: JSStringRef);

    // JSTypedArray.h
    pub fn JSObjectMakeTypedArray(
        ctx: JSContextRef,
        ty: JSTypedArrayType,
        length: usize,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeTypedArrayWithBytesNoCopy(
        ctx: JSContextRef,
        ty: JSTypedArrayType,
        bytes: *mut c_void,
        byte_length: usize,
        deallocator: JSTypedArrayBytesDeallocator,
        deallocator_context: *mut c_void,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectGetTypedArrayBytesPtr(
        ctx: JSContextRef,
        object: JSObjectRef,
        exception: *mut JSValueRef,
    ) -> *mut c_void;
    pub fn JSObjectGetTypedArrayLength(ctx: JSContextRef, object: JSObjectRef, exception: *mut JSValueRef) -> usize;
    pub fn JSObjectGetTypedArrayByteLength(ctx: JSContextRef, object: JSObjectRef, exception: *mut JSValueRef)
        -> usize;
    pub fn JSObjectGetTypedArrayByteOffset(ctx: JSContextRef, object: JSObjectRef, exception: *mut JSValueRef)
        -> usize;
    pub fn JSObjectGetTypedArrayBuffer(
        ctx: JSContextRef,
        object: JSObjectRef,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectMakeArrayBufferWithBytesNoCopy(
        ctx: JSContextRef,
        bytes: *mut c_void,
        byte_length: usize,
        deallocator: JSTypedArrayBytesDeallocator,
        deallocator_context: *mut c_void,
        exception: *mut JSValueRef,
    ) -> JSObjectRef;
    pub fn JSObjectGetArrayBufferBytesPtr(
        ctx: JSContextRef,
        object: JSObjectRef,
        exception: *mut JSValueRef,
    ) -> *mut c_void;
    pub fn JSObjectGetArrayBufferByteLength(
        ctx: JSContextRef,
        object: JSObjectRef,
        exception: *mut JSValueRef,
    ) -> usize;
}

/// Linked against the patched source build (`scripts/jsc/build.sh`) rather
/// than the system framework. Its timers run on WTF's own run loop, not on
/// `CFRunLoop`.
pub const SOURCE_BUILD: bool = cfg!(lp_jsc_source);

/// Functions exported by the system JavaScriptCore but declared only in
/// WebKit's private headers (`JSWeakPrivate.h`, `JSContextRefPrivate.h`,
/// `JSObjectRefPrivate.h`, `JSBasePrivate.h`). Signatures follow WebKit
/// `Source/JavaScriptCore/API`.
///
/// They are resolved at runtime with `dlsym`, never linked, so a JavaScriptCore
/// build without one of them still loads; callers must handle `None` and report
/// the capability as unsupported.
pub mod spi {
    use super::*;
    use std::sync::OnceLock;

    pub type JSShouldTerminateCallback = Option<unsafe extern "C" fn(ctx: JSContextRef, context: *mut c_void) -> bool>;

    pub type JSWeakCreateFn = unsafe extern "C" fn(group: JSContextGroupRef, object: JSObjectRef) -> JSWeakRef;
    /// A weak handle that also keeps its object alive while `*keep_alive` is
    /// nonzero (patched JSC, `JSWeakCreateKeptWhile`): the collector reads
    /// the byte while marking; the byte must outlive the handle.
    pub type JSWeakCreateKeptWhileFn =
        unsafe extern "C" fn(group: JSContextGroupRef, object: JSObjectRef, keep_alive: *const u8) -> JSWeakRef;
    pub type JSWeakReleaseFn = unsafe extern "C" fn(group: JSContextGroupRef, weak: JSWeakRef);
    /// `JSWeakRelease` of `count` handles under one lock (patched JSC).
    pub type JSWeakReleaseManyFn =
        unsafe extern "C" fn(group: JSContextGroupRef, weaks: *const JSWeakRef, count: usize);
    pub type JSWeakGetObjectFn = unsafe extern "C" fn(weak: JSWeakRef) -> JSObjectRef;
    /// The structure of new wrappers of a class with one prototype in one
    /// global object (patched JSC, `JSWrapperShapeCreate`); null if the class
    /// has none to share.
    pub type JSWrapperShapeRef = *mut c_void;
    pub type JSWrapperShapeCreateFn =
        unsafe extern "C" fn(ctx: JSContextRef, class: JSClassRef, prototype: JSObjectRef) -> JSWrapperShapeRef;
    pub type JSWrapperShapeReleaseFn = unsafe extern "C" fn(group: JSContextGroupRef, shape: JSWrapperShapeRef);
    /// A new wrapper of `shape` with private data `data` (`JSObjectMakeWrapper`):
    /// with `keep_alive`, also its `JSWeakCreateKeptWhile` handle in `*weak`.
    pub type JSObjectMakeWrapperFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        shape: JSWrapperShapeRef,
        data: *mut c_void,
        keep_alive: *const u8,
        weak: *mut JSWeakRef,
    ) -> JSObjectRef;
    /// A tracked wrapper (patch 0029, `JSObjectMakeTrackedWrapper`): an object
    /// of `class` (immortal) with private data `data`, whose cell has no
    /// destructor and no weak handle. `flags[0]` is the host's: while nonzero
    /// the object is kept alive. `flags[1]` is the collector's: nonzero once
    /// the object is dead. Both bytes must outlive the object's entry, which
    /// ends when `data` comes back from `JSContextGroupTakeDeadTrackedWrappers`.
    /// With `shape` (of `class`, of this global object) the object starts with
    /// its structure; otherwise it gets `prototype`.
    pub type JSObjectMakeTrackedWrapperFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        shape: JSWrapperShapeRef,
        class: JSClassRef,
        prototype: JSValueRef,
        data: *mut c_void,
        flags: *mut u8,
    ) -> JSObjectRef;
    /// Dead tracked wrappers not taken yet; any thread, no lock.
    pub type JSContextGroupDeadTrackedWrapperCounterFn =
        unsafe extern "C" fn(group: JSContextGroupRef) -> *const std::sync::atomic::AtomicUsize;
    /// The `data` of up to `capacity` dead (`...TakeDead...`) or of dead and
    /// live (`...TakeAll...`, which ends every entry) tracked wrappers.
    pub type JSContextGroupTakeTrackedWrappersFn =
        unsafe extern "C" fn(group: JSContextGroupRef, buffer: *mut *mut c_void, capacity: usize) -> usize;
    /// Clears `flags[0]` of every live tracked wrapper of `ctx`'s global object.
    pub type JSContextReleaseTrackedWrappersFn = unsafe extern "C" fn(ctx: JSContextRef);
    pub type JSGlobalContextSetUnhandledRejectionCallbackFn =
        unsafe extern "C" fn(ctx: JSGlobalContextRef, function: JSObjectRef, exception: *mut JSValueRef);
    pub type JSContextGroupSetExecutionTimeLimitFn = unsafe extern "C" fn(
        group: JSContextGroupRef,
        limit: c_double,
        callback: JSShouldTerminateCallback,
        context: *mut c_void,
    );
    pub type JSContextGroupClearExecutionTimeLimitFn = unsafe extern "C" fn(group: JSContextGroupRef);
    pub type JSObjectSetPrivatePropertyFn =
        unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef, value: JSValueRef) -> bool;
    pub type JSObjectGetPrivatePropertyFn =
        unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef) -> JSValueRef;
    pub type JSObjectDeletePrivatePropertyFn =
        unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef, name: JSStringRef) -> bool;
    pub type JSReportExtraMemoryCostFn = unsafe extern "C" fn(ctx: JSContextRef, size: usize);
    pub type JSSynchronousGarbageCollectForDebuggingFn = unsafe extern "C" fn(ctx: JSContextRef);
    pub type JSObjectGetProxyTargetFn = unsafe extern "C" fn(object: JSObjectRef) -> JSObjectRef;
    pub type JSObjectGetGlobalContextFn = unsafe extern "C" fn(object: JSObjectRef) -> JSGlobalContextRef;
    /// Patched JavaScriptCore only (`webkit/patches/0002-fork-support.patch`).
    pub type JSPrepareForForkFn = unsafe extern "C" fn(group: JSContextGroupRef, timeout_seconds: c_double) -> bool;
    pub type JSDidForkFn = unsafe extern "C" fn(is_child: bool);
    pub type JSGlobalContextReseedRandomFn = unsafe extern "C" fn(ctx: JSGlobalContextRef);
    /// Patched JavaScriptCore only (`webkit/patches/0003-complete-jit-plans.patch`).
    pub type JSCompleteJITPlansFn = unsafe extern "C" fn(group: JSContextGroupRef, wait: bool);
    /// Patched JavaScriptCore only (`webkit/patches/0006-vm-time-zone.patch`).
    pub type JSSetWrapperPropertyAddedCallbackFn =
        unsafe extern "C" fn(callback: Option<unsafe extern "C" fn(JSContextRef, JSObjectRef)>);
    pub type JSContextGroupSetTimeZoneFn = unsafe extern "C" fn(group: JSContextGroupRef, id: *const c_char) -> bool;
    pub type JSNamedPropertyFilter =
        unsafe extern "C" fn(ctx: JSContextRef, characters: *const c_void, length: usize, is_8bit: bool) -> bool;
    /// Patched JavaScriptCore only (`webkit/patches/0015-impure-named-properties.patch`).
    pub type JSAddImpurePropertyFn = unsafe extern "C" fn(ctx: JSContextRef, name: JSStringRef);
    /// Patched JavaScriptCore only (`webkit/patches/0004-named-properties-object.patch`).
    pub type JSGlobalContextInstallNamedPropertiesFn = unsafe extern "C" fn(
        ctx: JSGlobalContextRef,
        filter: JSNamedPropertyFilter,
        getter: JSObjectGetPropertyCallback,
    );

    /// A custom accessor's getter (patched JSC, `JSObjectSetCustomAccessor`):
    /// called by inline caches with the receiver as `this_value` (any value,
    /// not only objects of the class) and the property name's unique string
    /// pointer. Throws with `JSContextThrowException` and returns undefined.
    pub type JSCustomGetterCallback =
        unsafe extern "C" fn(ctx: JSContextRef, this_value: JSValueRef, uid: *const c_void) -> JSValueRef;
    pub type JSCustomSetterCallback =
        unsafe extern "C" fn(ctx: JSContextRef, this_value: JSValueRef, value: JSValueRef, uid: *const c_void) -> bool;
    pub type JSObjectSetCustomAccessorFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        getter: Option<JSCustomGetterCallback>,
        setter: Option<JSCustomSetterCallback>,
        attributes: JSPropertyAttributes,
    ) -> bool;
    /// A constant accessor (patched JSC, `JSObjectSetConstantAccessor`): a
    /// non-configurable accessor of `object` whose reads on it yield `value`
    /// with no callback, and which the DFG folds to `value`. Set once.
    pub type JSObjectSetConstantAccessorFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        value: JSValueRef,
        setter: Option<JSCustomSetterCallback>,
        attributes: JSPropertyAttributes,
    ) -> bool;
    /// What makes a lazy constant accessor's value on its first read
    /// (`JSConstantAccessorInitializer`): `ctx` is the realm of `object`, the
    /// holder. The value, or null with `*exception` set.
    pub type JSConstantAccessorInitializer = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    /// Sets the process's initializer of lazy constant accessors (patched JSC,
    /// `JSSetConstantAccessorInitializer`).
    pub type JSSetConstantAccessorInitializerFn =
        unsafe extern "C" fn(initializer: Option<JSConstantAccessorInitializer>);
    /// A constant accessor whose value the initializer makes on the first
    /// read (patched JSC, `JSObjectSetLazyConstantAccessor`): the same
    /// non-configurable property from the start. Set once.
    pub type JSObjectSetLazyConstantAccessorFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        setter: Option<JSCustomSetterCallback>,
        attributes: JSPropertyAttributes,
    ) -> bool;
    /// Releases the values of `object`'s constant accessors as its realm is
    /// torn down (patched JSC, `JSObjectClearConstantAccessors`): reads of
    /// them throw from then on, lazy ones' too.
    pub type JSObjectClearConstantAccessorsFn = unsafe extern "C" fn(ctx: JSContextRef, object: JSObjectRef);
    pub type JSContextThrowExceptionFn = unsafe extern "C" fn(ctx: JSContextRef, exception: JSValueRef);
    /// A fast getter (patched JSC, `JSObjectSetFastGetter`): what the JITs
    /// call in place of the custom getter once they have proved the receiver
    /// is a C API wrapper object, with its private data and `context`.
    pub type JSFastGetterCallback = unsafe extern "C" fn(
        ctx: JSContextRef,
        this_object: JSObjectRef,
        private_data: *mut c_void,
        context: *mut c_void,
    ) -> JSValueRef;
    pub const kJSFastGetterResultAny: c_int = 0;
    pub const kJSFastGetterResultNumber: c_int = 1;
    pub const kJSFastGetterResultBoolean: c_int = 2;
    pub const kJSFastGetterResultString: c_int = 3;
    pub const kJSFastGetterResultObjectOrNull: c_int = 4;
    pub type JSObjectSetFastGetterFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        name: JSStringRef,
        getter: Option<JSCustomGetterCallback>,
        setter: Option<JSCustomSetterCallback>,
        attributes: JSPropertyAttributes,
        fast_getter: Option<JSFastGetterCallback>,
        context: *mut c_void,
        result_type: c_int,
        reads_only: bool,
    ) -> bool;
    /// Indexed property callbacks of a class (`JSClassSetIndexedCallbacks`).
    pub type JSObjectGetPropertyByIndexCallback = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        index: c_uint,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub type JSObjectSetPropertyByIndexCallback = unsafe extern "C" fn(
        ctx: JSContextRef,
        object: JSObjectRef,
        index: c_uint,
        value: JSValueRef,
        exception: *mut JSValueRef,
    ) -> bool;
    pub type JSClassSetIndexedCallbacksFn = unsafe extern "C" fn(
        class: JSClassRef,
        get: Option<JSObjectGetPropertyByIndexCallback>,
        set: Option<JSObjectSetPropertyByIndexCallback>,
    );
    /// Names a class's wrappers have as own properties while the
    /// prototype's accessors serve reads (`JSClassSetOwnPropertyNames`,
    /// patch 0024). `get_value` gives an answered name's value when a
    /// descriptor or a copy reads it (null, or no callback: a `[[Get]]`).
    pub const kJSOwnPropertyNameListed: u8 = 1;
    pub const kJSOwnPropertyNameDontEnum: u8 = 2;
    pub const kJSOwnPropertyNameAnswered: u8 = 4;
    pub type JSClassSetOwnPropertyNamesFn = unsafe extern "C" fn(
        class: JSClassRef,
        names: *const *const c_char,
        flags: *const u8,
        count: usize,
        get_value: JSObjectGetPropertyCallback,
    );
    /// A function object with embedder data (`JSObjectMakeFunctionWithCallbackAndData`).
    pub type JSObjectCallAsFunctionWithDataCallback = unsafe extern "C" fn(
        ctx: JSContextRef,
        function: JSObjectRef,
        this: JSObjectRef,
        argc: usize,
        argv: *const JSValueRef,
        data: *mut c_void,
        exception: *mut JSValueRef,
    ) -> JSValueRef;
    pub type JSObjectMakeFunctionWithCallbackAndDataFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        name: JSStringRef,
        length: c_uint,
        callback: Option<JSObjectCallAsFunctionWithDataCallback>,
        data: *mut c_void,
    ) -> JSObjectRef;
    pub type JSObjectGetFunctionDataFn = unsafe extern "C" fn(object: JSObjectRef) -> *mut c_void;
    pub type JSContextGroupReleaseFreeMemoryFn = unsafe extern "C" fn(group: JSContextGroupRef);
    pub type JSContextGroupDeleteAllCodeFn = unsafe extern "C" fn(group: JSContextGroupRef);
    /// Freed allocator pages are replaced by fresh mappings (they leave the
    /// resident set at once) instead of being `madvise`d.
    pub type JSSetDecommitReplacesPagesFn = unsafe extern "C" fn(enable: bool);
    /// A string value's characters without a copy: `length` units, Latin-1
    /// bytes when `is_8bit`, else UTF-16. Valid while the value is reachable
    /// and no JavaScript runs; null for a non-string.
    pub type JSValueGetStringCharactersFn = unsafe extern "C" fn(
        ctx: JSContextRef,
        value: JSValueRef,
        length: *mut usize,
        is_8bit: *mut bool,
    ) -> *const c_void;
    /// A string value made from `length` Latin-1 bytes, copied once into the
    /// string (patched JSC, `webkit/patches/0016-direct-strings.patch`):
    /// `JSValueMakeString` without the `JSStringRef` and its two copies. Null
    /// when `length` exceeds JavaScript's longest string. Takes the API lock
    /// only when the caller does not hold it.
    pub type JSValueMakeStringLatin1Fn =
        unsafe extern "C" fn(ctx: JSContextRef, characters: *const u8, length: usize) -> JSValueRef;
    /// [`JSValueMakeStringLatin1Fn`] for UTF-16 code units (unpaired
    /// surrogates kept).
    pub type JSValueMakeStringUTF16Fn =
        unsafe extern "C" fn(ctx: JSContextRef, characters: *const JSChar, length: usize) -> JSValueRef;
    /// `JSStringCreateWithCharacters` for Latin-1 bytes: an 8-bit string,
    /// nothing decoded (patched JSC, 0016).
    pub type JSStringCreateWithLatin1CharactersFn =
        unsafe extern "C" fn(characters: *const u8, length: usize) -> JSStringRef;
    /// A string's characters as it stores them (patched JSC, 0016): `length`
    /// units, Latin-1 bytes when `is_8bit`, else UTF-16, valid while the
    /// string is retained (`JSStringGetCharactersPtr` widens an 8-bit string
    /// into a second copy the string keeps). May be null when empty.
    pub type JSStringGetStoredCharactersFn =
        unsafe extern "C" fn(string: JSStringRef, length: *mut usize, is_8bit: *mut bool) -> *const c_void;

    /// A source of block coverage (patched JSC, `webkit/patches/0019-block-coverage.patch`):
    /// its id, the global object it first ran in (an identity only), URL (a
    /// `//# sourceURL=` comment wins), the URL of the code that made it (eval,
    /// `new Function`; may be null), text (null from `JSContextGroupTakeCoverage`)
    /// and whether it is a module. Valid for the callback only.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct JSCoverageSource {
        pub source_id: isize,
        pub global: JSGlobalContextRef,
        pub url: JSStringRef,
        pub origin_url: JSStringRef,
        pub source: JSStringRef,
        pub is_module: bool,
    }
    /// One basic block: UTF-16 source offsets, `end` inclusive, and how often it ran.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct JSCoverageBlock {
        pub start: i32,
        pub end: i32,
        pub count: u64,
    }
    /// A function declared by code compiled while coverage was on: `end` is
    /// its closing brace; `name` is null when anonymous.
    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct JSCoverageFunction {
        pub start: u32,
        pub end: u32,
        pub executed: bool,
        pub name: JSStringRef,
    }
    pub type JSCoverageSourceCallback = unsafe extern "C" fn(
        data: *mut c_void,
        source: *const JSCoverageSource,
        blocks: *const JSCoverageBlock,
        block_count: usize,
        functions: *const JSCoverageFunction,
        function_count: usize,
    );
    /// Counted on/off of block coverage for a VM; the first enable and the
    /// last disable recompile all code once no JavaScript runs.
    pub type JSContextGroupSetCoverageEnabledFn = unsafe extern "C" fn(group: JSContextGroupRef, enabled: bool);
    /// The sources first compiled after the first `*cursor` ones, with text.
    pub type JSContextGroupCoverageNewSourcesFn = unsafe extern "C" fn(
        group: JSContextGroupRef,
        cursor: *mut usize,
        callback: JSCoverageSourceCallback,
        data: *mut c_void,
    );
    /// Every source with what ran, without text.
    pub type JSContextGroupTakeCoverageFn =
        unsafe extern "C" fn(group: JSContextGroupRef, callback: JSCoverageSourceCallback, data: *mut c_void);

    /// See [`Spi::tracked`].
    #[derive(Debug, Clone, Copy)]
    pub struct TrackedWrappers {
        pub make: JSObjectMakeTrackedWrapperFn,
        pub dead_counter: JSContextGroupDeadTrackedWrapperCounterFn,
        pub take_dead: JSContextGroupTakeTrackedWrappersFn,
        pub take_all: JSContextGroupTakeTrackedWrappersFn,
        pub release: JSContextReleaseTrackedWrappersFn,
    }

    /// Runtime-resolved SPI table. Each entry is `None` when the running
    /// JavaScriptCore does not export the symbol.
    #[derive(Debug, Clone, Copy)]
    pub struct Spi {
        pub weak_create: Option<JSWeakCreateFn>,
        pub weak_create_kept_while: Option<JSWeakCreateKeptWhileFn>,
        pub weak_release: Option<JSWeakReleaseFn>,
        pub weak_release_many: Option<JSWeakReleaseManyFn>,
        pub weak_get_object: Option<JSWeakGetObjectFn>,
        pub wrapper_shape_create: Option<JSWrapperShapeCreateFn>,
        pub wrapper_shape_release: Option<JSWrapperShapeReleaseFn>,
        pub object_make_wrapper: Option<JSObjectMakeWrapperFn>,
        pub object_make_tracked_wrapper: Option<JSObjectMakeTrackedWrapperFn>,
        pub dead_tracked_wrapper_counter: Option<JSContextGroupDeadTrackedWrapperCounterFn>,
        pub take_dead_tracked_wrappers: Option<JSContextGroupTakeTrackedWrappersFn>,
        pub take_all_tracked_wrappers: Option<JSContextGroupTakeTrackedWrappersFn>,
        pub release_tracked_wrappers: Option<JSContextReleaseTrackedWrappersFn>,
        pub set_unhandled_rejection_callback: Option<JSGlobalContextSetUnhandledRejectionCallbackFn>,
        pub set_execution_time_limit: Option<JSContextGroupSetExecutionTimeLimitFn>,
        pub clear_execution_time_limit: Option<JSContextGroupClearExecutionTimeLimitFn>,
        pub set_private_property: Option<JSObjectSetPrivatePropertyFn>,
        pub get_private_property: Option<JSObjectGetPrivatePropertyFn>,
        pub delete_private_property: Option<JSObjectDeletePrivatePropertyFn>,
        pub report_extra_memory_cost: Option<JSReportExtraMemoryCostFn>,
        pub synchronous_gc_for_debugging: Option<JSSynchronousGarbageCollectForDebuggingFn>,
        pub get_proxy_target: Option<JSObjectGetProxyTargetFn>,
        pub get_object_global_context: Option<JSObjectGetGlobalContextFn>,
        pub prepare_for_fork: Option<JSPrepareForForkFn>,
        pub did_fork: Option<JSDidForkFn>,
        pub reseed_random: Option<JSGlobalContextReseedRandomFn>,
        pub complete_jit_plans: Option<JSCompleteJITPlansFn>,
        pub set_time_zone: Option<JSContextGroupSetTimeZoneFn>,
        pub set_wrapper_property_added: Option<JSSetWrapperPropertyAddedCallbackFn>,
        pub install_named_properties: Option<JSGlobalContextInstallNamedPropertiesFn>,
        pub add_impure_property: Option<JSAddImpurePropertyFn>,
        pub set_custom_accessor: Option<JSObjectSetCustomAccessorFn>,
        pub set_fast_getter: Option<JSObjectSetFastGetterFn>,
        pub set_constant_accessor: Option<JSObjectSetConstantAccessorFn>,
        pub set_lazy_constant_accessor: Option<JSObjectSetLazyConstantAccessorFn>,
        pub set_constant_accessor_initializer: Option<JSSetConstantAccessorInitializerFn>,
        pub clear_constant_accessors: Option<JSObjectClearConstantAccessorsFn>,
        pub throw_exception: Option<JSContextThrowExceptionFn>,
        pub class_set_indexed_callbacks: Option<JSClassSetIndexedCallbacksFn>,
        pub class_set_own_property_names: Option<JSClassSetOwnPropertyNamesFn>,
        pub make_function_with_data: Option<JSObjectMakeFunctionWithCallbackAndDataFn>,
        pub get_function_data: Option<JSObjectGetFunctionDataFn>,
        pub release_free_memory: Option<JSContextGroupReleaseFreeMemoryFn>,
        pub delete_all_code: Option<JSContextGroupDeleteAllCodeFn>,
        pub set_decommit_replaces_pages: Option<JSSetDecommitReplacesPagesFn>,
        pub value_string_characters: Option<JSValueGetStringCharactersFn>,
        pub make_string_latin1: Option<JSValueMakeStringLatin1Fn>,
        pub make_string_utf16: Option<JSValueMakeStringUTF16Fn>,
        pub string_create_latin1: Option<JSStringCreateWithLatin1CharactersFn>,
        pub string_stored_characters: Option<JSStringGetStoredCharactersFn>,
        pub set_coverage_enabled: Option<JSContextGroupSetCoverageEnabledFn>,
        pub coverage_new_sources: Option<JSContextGroupCoverageNewSourcesFn>,
        pub take_coverage: Option<JSContextGroupTakeCoverageFn>,
    }

    impl Spi {
        /// Weak references (wrapper identity maps) need all three entry points.
        pub fn weak(&self) -> Option<(JSWeakCreateFn, JSWeakReleaseFn, JSWeakGetObjectFn)> {
            Some((self.weak_create?, self.weak_release?, self.weak_get_object?))
        }

        /// Tracked wrappers (patch 0029), all or nothing.
        pub fn tracked(&self) -> Option<TrackedWrappers> {
            Some(TrackedWrappers {
                make: self.object_make_tracked_wrapper?,
                dead_counter: self.dead_tracked_wrapper_counter?,
                take_dead: self.take_dead_tracked_wrappers?,
                take_all: self.take_all_tracked_wrappers?,
                release: self.release_tracked_wrappers?,
            })
        }

        /// Strings to and from host characters without a `JSStringRef` in
        /// between (patch 0016), all or nothing. Its JavaScriptCore also makes
        /// `value_string_characters` safe anywhere: a flat string is read with
        /// no lock and no exception check, and a rope is resolved under the API
        /// lock, taken when the caller does not hold it; a failure there (out of
        /// memory) leaves no exception pending.
        pub fn direct_strings(&self) -> Option<DirectStrings> {
            self.value_string_characters?;
            Some(DirectStrings {
                make_latin1: self.make_string_latin1?,
                make_utf16: self.make_string_utf16?,
                create_latin1: self.string_create_latin1?,
                stored_characters: self.string_stored_characters?,
            })
        }

        /// Block coverage (patch 0019) needs all three entry points.
        pub fn coverage(
            &self,
        ) -> Option<(
            JSContextGroupSetCoverageEnabledFn,
            JSContextGroupCoverageNewSourcesFn,
            JSContextGroupTakeCoverageFn,
        )> {
            Some((self.set_coverage_enabled?, self.coverage_new_sources?, self.take_coverage?))
        }

        /// fork() support needs all three entry points.
        pub fn fork(&self) -> Option<(JSPrepareForForkFn, JSDidForkFn, JSGlobalContextReseedRandomFn)> {
            Some((self.prepare_for_fork?, self.did_fork?, self.reseed_random?))
        }
    }

    /// The entry points of [`Spi::direct_strings`].
    #[derive(Debug, Clone, Copy)]
    pub struct DirectStrings {
        pub make_latin1: JSValueMakeStringLatin1Fn,
        pub make_utf16: JSValueMakeStringUTF16Fn,
        pub create_latin1: JSStringCreateWithLatin1CharactersFn,
        pub stored_characters: JSStringGetStoredCharactersFn,
    }

    /// [`Spi::direct_strings`], resolved once per process: one load on the
    /// paths that make every string.
    #[inline]
    pub fn direct_strings() -> Option<&'static DirectStrings> {
        static DIRECT: OnceLock<Option<DirectStrings>> = OnceLock::new();
        DIRECT.get_or_init(|| get().direct_strings()).as_ref()
    }

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    #[cfg(target_vendor = "apple")]
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;
    #[cfg(not(target_vendor = "apple"))]
    const RTLD_DEFAULT: *mut c_void = std::ptr::null_mut();

    macro_rules! resolve {
        ($name:literal) => {{
            // SAFETY: dlsym with RTLD_DEFAULT and a NUL-terminated name; the
            // result is either null or the address of the named exported
            // function, whose signature is declared by the target fn type.
            let p = unsafe { dlsym(RTLD_DEFAULT, concat!($name, "\0").as_ptr().cast()) };
            if p.is_null() {
                None
            } else {
                Some(unsafe { core::mem::transmute::<*mut c_void, _>(p) })
            }
        }};
    }

    /// Resolves the SPI table once per process.
    #[allow(clippy::missing_transmute_annotations)]
    pub fn get() -> &'static Spi {
        static SPI: OnceLock<Spi> = OnceLock::new();
        SPI.get_or_init(|| Spi {
            weak_create: resolve!("JSWeakCreate"),
            weak_create_kept_while: resolve!("JSWeakCreateKeptWhile"),
            weak_release: resolve!("JSWeakRelease"),
            weak_release_many: resolve!("JSWeakReleaseMany"),
            weak_get_object: resolve!("JSWeakGetObject"),
            wrapper_shape_create: resolve!("JSWrapperShapeCreate"),
            wrapper_shape_release: resolve!("JSWrapperShapeRelease"),
            object_make_wrapper: resolve!("JSObjectMakeWrapper"),
            object_make_tracked_wrapper: resolve!("JSObjectMakeTrackedWrapper"),
            dead_tracked_wrapper_counter: resolve!("JSContextGroupDeadTrackedWrapperCounter"),
            take_dead_tracked_wrappers: resolve!("JSContextGroupTakeDeadTrackedWrappers"),
            take_all_tracked_wrappers: resolve!("JSContextGroupTakeAllTrackedWrappers"),
            release_tracked_wrappers: resolve!("JSContextReleaseTrackedWrappers"),
            set_unhandled_rejection_callback: resolve!("JSGlobalContextSetUnhandledRejectionCallback"),
            set_execution_time_limit: resolve!("JSContextGroupSetExecutionTimeLimit"),
            clear_execution_time_limit: resolve!("JSContextGroupClearExecutionTimeLimit"),
            set_private_property: resolve!("JSObjectSetPrivateProperty"),
            get_private_property: resolve!("JSObjectGetPrivateProperty"),
            delete_private_property: resolve!("JSObjectDeletePrivateProperty"),
            report_extra_memory_cost: resolve!("JSReportExtraMemoryCost"),
            synchronous_gc_for_debugging: resolve!("JSSynchronousGarbageCollectForDebugging"),
            get_proxy_target: resolve!("JSObjectGetProxyTarget"),
            get_object_global_context: resolve!("JSObjectGetGlobalContext"),
            prepare_for_fork: resolve!("JSPrepareForFork"),
            did_fork: resolve!("JSDidFork"),
            reseed_random: resolve!("JSGlobalContextReseedRandom"),
            complete_jit_plans: resolve!("JSCompleteJITPlans"),
            set_time_zone: resolve!("JSContextGroupSetTimeZone"),
            set_wrapper_property_added: resolve!("JSSetWrapperPropertyAddedCallback"),
            install_named_properties: resolve!("JSGlobalContextInstallNamedProperties"),
            add_impure_property: resolve!("JSAddImpureProperty"),
            set_custom_accessor: resolve!("JSObjectSetCustomAccessor"),
            set_fast_getter: resolve!("JSObjectSetFastGetter"),
            set_constant_accessor: resolve!("JSObjectSetConstantAccessor"),
            set_lazy_constant_accessor: resolve!("JSObjectSetLazyConstantAccessor"),
            set_constant_accessor_initializer: resolve!("JSSetConstantAccessorInitializer"),
            clear_constant_accessors: resolve!("JSObjectClearConstantAccessors"),
            throw_exception: resolve!("JSContextThrowException"),
            class_set_indexed_callbacks: resolve!("JSClassSetIndexedCallbacks"),
            class_set_own_property_names: resolve!("JSClassSetOwnPropertyNames"),
            make_function_with_data: resolve!("JSObjectMakeFunctionWithCallbackAndData"),
            get_function_data: resolve!("JSObjectGetFunctionData"),
            release_free_memory: resolve!("JSContextGroupReleaseFreeMemory"),
            delete_all_code: resolve!("JSContextGroupDeleteAllCode"),
            set_decommit_replaces_pages: resolve!("JSSetDecommitReplacesPages"),
            value_string_characters: resolve!("JSValueGetStringCharacters"),
            make_string_latin1: resolve!("JSValueMakeStringLatin1"),
            make_string_utf16: resolve!("JSValueMakeStringUTF16"),
            string_create_latin1: resolve!("JSStringCreateWithLatin1Characters"),
            string_stored_characters: resolve!("JSStringGetStoredCharacters"),
            set_coverage_enabled: resolve!("JSContextGroupSetCoverageEnabled"),
            coverage_new_sources: resolve!("JSContextGroupCoverageNewSources"),
            take_coverage: resolve!("JSContextGroupTakeCoverage"),
        })
    }
}

/// ES module loading through the Objective-C SPI shim (`shim/modules.m`).
pub mod modules {
    use super::*;

    pub type lp_module_fetch_cb = unsafe extern "C" fn(
        userdata: *mut c_void,
        ctx: JSGlobalContextRef,
        identifier: JSStringRef,
        module_type: c_int,
        resolve: JSObjectRef,
        reject: JSObjectRef,
    );

    /// `module_type` values of [`lp_module_fetch_cb`] and
    /// [`lp_jsc_module_script_create`] (the request's `type` import attribute).
    pub const MODULE_TYPE_JAVASCRIPT: c_int = 0;
    pub const MODULE_TYPE_JSON: c_int = 1;
    pub const MODULE_TYPE_CSS: c_int = 2;
    pub const MODULE_TYPE_WEBASSEMBLY: c_int = 3;
    pub const MODULE_TYPE_TEXT: c_int = 4;

    /// The document base URL as an owned string, or null.
    pub type lp_module_base_url_cb = unsafe extern "C" fn(ctx: JSGlobalContextRef) -> JSStringRef;

    /// Counters of a context group's precompiled modules
    /// ([`lp_jsc_module_precompile_stats`]). A forked process starts from its
    /// parent's values.
    #[repr(C)]
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub struct ModulePrecompileStats {
        /// What [`lp_jsc_module_precompile`] keeps: modules, the functions
        /// compiled with them, and the length of their sources.
        pub modules: usize,
        pub functions: usize,
        pub characters: usize,
        /// Module loads that found their source precompiled, and those that
        /// did not, since the group has had a precompiled module.
        pub record_hits: usize,
        pub record_misses: usize,
        /// Module evaluations that took their code from a precompiled module.
        pub code_hits: usize,
        /// Functions the group compiled to bytecode, those of
        /// [`lp_jsc_module_precompile`] included.
        pub function_compiles: usize,
    }

    unsafe extern "C" {
        /// 1 when the module SPI is present (always check before other calls).
        pub fn lp_jsc_modules_available() -> c_int;
        pub fn lp_jsc_module_loader_install(
            ctx: JSGlobalContextRef,
            cb: lp_module_fetch_cb,
            base_url: lp_module_base_url_cb,
            userdata: *mut c_void,
        ) -> *mut c_void;
        pub fn lp_jsc_module_loader_release(handle: *mut c_void);
        /// 1 when `lp_jsc_import_map_add` works (source-built JavaScriptCore).
        pub fn lp_jsc_import_maps_available() -> c_int;
        /// 1 when fetch callbacks see the `type` import attribute and JSON
        /// and CSS module scripts can load (source-built JavaScriptCore).
        pub fn lp_jsc_typed_modules_available() -> c_int;
        pub fn lp_jsc_import_map_add(
            ctx: JSGlobalContextRef,
            src: *const c_char,
            src_len: usize,
            base: *const c_char,
            base_len: usize,
            messages: *mut JSStringRef,
        ) -> c_int;
        pub fn lp_jsc_module_script_create(
            ctx: JSGlobalContextRef,
            src: *const c_char,
            src_len: usize,
            url: *const c_char,
            url_len: usize,
            module_type: c_int,
            error: *mut JSStringRef,
        ) -> JSValueRef;
        pub fn lp_jsc_module_evaluate(
            ctx: JSGlobalContextRef,
            src: *const c_char,
            src_len: usize,
            url: *const c_char,
            url_len: usize,
            error: *mut JSStringRef,
        ) -> JSValueRef;
        /// 1 when `lp_jsc_module_precompile` works (source-built JavaScriptCore).
        pub fn lp_jsc_module_precompile_available() -> c_int;
        /// Parses and compiles, ahead of its first load, the JavaScript module
        /// source that `lp_jsc_module_script_create` makes for the same `src`
        /// and `url` (the code of every function in it too when `functions`
        /// is not 0), and keeps it for the lifetime of the context's group.
        /// A load of the same text from a URL of the same host, in any
        /// context of the group or of a process forked from this one, then
        /// parses and compiles nothing. Runs nothing of the module. Returns 1
        /// when the module is kept, 0 for a source that does not parse.
        pub fn lp_jsc_module_precompile(
            ctx: JSGlobalContextRef,
            src: *const c_char,
            src_len: usize,
            url: *const c_char,
            url_len: usize,
            functions: c_int,
        ) -> c_int;
        /// All zero without `lp_jsc_module_precompile_available`.
        pub fn lp_jsc_module_precompile_stats(group: JSContextGroupRef, stats: *mut ModulePrecompileStats);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn evaluates_script_in_system_jsc() {
        unsafe {
            let group = JSContextGroupCreate();
            let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null());
            let src = CString::new("[1,2,3].map(x => x * 2).join(',')").unwrap();
            let script = JSStringCreateWithUTF8CString(src.as_ptr());
            let mut exc: JSValueRef = core::ptr::null();
            let v = JSEvaluateScript(ctx, script, core::ptr::null_mut(), core::ptr::null(), 1, &mut exc);
            JSStringRelease(script);
            assert!(exc.is_null());
            let s = JSValueToStringCopy(ctx, v, core::ptr::null_mut());
            let mut buf = vec![0u8; JSStringGetMaximumUTF8CStringSize(s)];
            let n = JSStringGetUTF8CString(s, buf.as_mut_ptr().cast(), buf.len());
            JSStringRelease(s);
            assert_eq!(&buf[..n - 1], b"2,4,6");
            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
        }
    }

    #[test]
    fn spi_weak_reference_roundtrip() {
        unsafe {
            let group = JSContextGroupCreate();
            let ctx = JSGlobalContextCreateInGroup(group, core::ptr::null());
            let obj = JSObjectMake(ctx, core::ptr::null(), core::ptr::null_mut());
            let (create, release, get) = spi::get().weak().expect("JSWeak SPI");
            let weak = create(group, obj);
            assert_eq!(get(weak), obj);
            release(group, weak);
            JSGlobalContextRelease(ctx);
            JSContextGroupRelease(group);
        }
    }
}

/// Objective-C autorelease boundaries for native threads entering JavaScriptCore.
#[cfg(target_os = "macos")]
pub mod autorelease {
    use std::os::raw::c_void;

    #[link(name = "objc")]
    unsafe extern "C" {
        /// Push a pool on this thread. Pop it in LIFO order before thread exit.
        pub fn objc_autoreleasePoolPush() -> *mut c_void;
        /// Drain a pool created on the same thread with no younger live pools.
        pub fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
}

/// Foundation class initialization needed before a macOS process first forks.
#[cfg(target_os = "macos")]
pub mod foundation {
    unsafe extern "C" {
        /// Materializes and validates NSNumber's boolean/integer/floating class
        /// clusters in an autorelease pool. Call in the parent before its first
        /// fork; returns 1 when the number values round-trip correctly.
        pub fn lp_foundation_initialize_numbers() -> std::os::raw::c_int;
    }
}
