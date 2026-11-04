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
    pub type JSWeakReleaseFn = unsafe extern "C" fn(group: JSContextGroupRef, weak: JSWeakRef);
    pub type JSWeakGetObjectFn = unsafe extern "C" fn(weak: JSWeakRef) -> JSObjectRef;
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
    pub type JSNamedPropertyFilter =
        unsafe extern "C" fn(ctx: JSContextRef, characters: *const c_void, length: usize, is_8bit: bool) -> bool;
    /// Patched JavaScriptCore only (`webkit/patches/0004-named-properties-object.patch`).
    pub type JSGlobalContextInstallNamedPropertiesFn = unsafe extern "C" fn(
        ctx: JSGlobalContextRef,
        filter: JSNamedPropertyFilter,
        getter: JSObjectGetPropertyCallback,
    );

    /// Runtime-resolved SPI table. Each entry is `None` when the running
    /// JavaScriptCore does not export the symbol.
    #[derive(Debug, Clone, Copy)]
    pub struct Spi {
        pub weak_create: Option<JSWeakCreateFn>,
        pub weak_release: Option<JSWeakReleaseFn>,
        pub weak_get_object: Option<JSWeakGetObjectFn>,
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
        pub install_named_properties: Option<JSGlobalContextInstallNamedPropertiesFn>,
    }

    impl Spi {
        /// Weak references (wrapper identity maps) need all three entry points.
        pub fn weak(&self) -> Option<(JSWeakCreateFn, JSWeakReleaseFn, JSWeakGetObjectFn)> {
            Some((self.weak_create?, self.weak_release?, self.weak_get_object?))
        }

        /// fork() support needs all three entry points.
        pub fn fork(&self) -> Option<(JSPrepareForForkFn, JSDidForkFn, JSGlobalContextReseedRandomFn)> {
            Some((self.prepare_for_fork?, self.did_fork?, self.reseed_random?))
        }
    }

    unsafe extern "C" {
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    const RTLD_DEFAULT: *mut c_void = -2isize as *mut c_void;

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
            weak_release: resolve!("JSWeakRelease"),
            weak_get_object: resolve!("JSWeakGetObject"),
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
            install_named_properties: resolve!("JSGlobalContextInstallNamedProperties"),
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
        resolve: JSObjectRef,
        reject: JSObjectRef,
    );

    unsafe extern "C" {
        /// 1 when the module SPI is present (always check before other calls).
        pub fn lp_jsc_modules_available() -> c_int;
        pub fn lp_jsc_module_loader_install(
            ctx: JSGlobalContextRef,
            cb: lp_module_fetch_cb,
            userdata: *mut c_void,
        ) -> *mut c_void;
        pub fn lp_jsc_module_loader_release(handle: *mut c_void);
        pub fn lp_jsc_module_script_create(
            ctx: JSGlobalContextRef,
            src: *const c_char,
            src_len: usize,
            url: *const c_char,
            url_len: usize,
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
pub mod foundation {
    unsafe extern "C" {
        /// Materializes and validates NSNumber's boolean/integer/floating class
        /// clusters in an autorelease pool. Call in the parent before its first
        /// fork; returns 1 when the number values round-trip correctly.
        pub fn lp_foundation_initialize_numbers() -> std::os::raw::c_int;
    }
}
