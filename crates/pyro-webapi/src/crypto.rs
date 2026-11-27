//! Web Crypto: `crypto.getRandomValues`, `crypto.randomUUID`,
//! `crypto.subtle.digest` (SHA-1/256/384/512, native) and the other
//! SubtleCrypto operations with `CryptoKey`, which run in subtle.js over the
//! primitives of crypto_subtle.rs. That script is made on a realm's first
//! SubtleCrypto call other than `digest` or its first read of `CryptoKey`, so
//! realms that never use keys pay one global accessor.
//!
//! `getRandomValues` writes into the caller's typed array through the JSC C
//! API (backing store pointer + byteOffset); the pointer is obtained and
//! used with no intervening JS call.

use pyro_js::sys;
use std::rc::Rc;

use pyro_js::{Args, ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Object, Persistent, Scope, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::page::PageDom;
use crate::util::*;
use crate::window::WindowHost;

/// QuotaExceededError limit of `getRandomValues` (Web Crypto §10.1.1).
const MAX_RANDOM_BYTES: usize = 65536;

fn check_exc(s: &Scope<'_>, exc: sys::JSValueRef) -> JsResult<()> {
    if exc.is_null() {
        return Ok(());
    }
    // SAFETY: `exc` is the live exception JSC just returned for this context.
    let v = unsafe { Value::from_raw(s.ctx(), exc) };
    Err(JsError::thrown(pyro_js::Persistent::new(s, v)))
}

fn fill_random(buf: &mut [u8]) -> JsResult<()> {
    getrandom::fill(buf).map_err(|e| JsError::dom("OperationError", format!("random source failed: {e}")))
}

/// A random 64-bit value from the OS generator (salts that must differ between frames and not be guessable).
pub fn random_u64() -> u64 {
    getrandom::u64().expect("the OS random number generator failed")
}

/// `crypto.getRandomValues(array)`: integer typed arrays only.
fn get_random_values<'s>(s: &Scope<'s>, v: Value<'s>) -> JsResult<Value<'s>> {
    let obj = v.as_object().ok_or_else(|| {
        JsError::Type(
            "Failed to execute 'getRandomValues' on 'Crypto': parameter 1 is not of type 'ArrayBufferView'.".into(),
        )
    })?;
    let ctx = s.ctx();
    let mut exc = std::ptr::null();
    // SAFETY: `obj` is rooted by the caller's argument frame.
    let ty = unsafe { sys::JSValueGetTypedArrayType(ctx, obj.as_raw() as sys::JSValueRef, &mut exc) };
    check_exc(s, exc)?;
    let integer = matches!(
        ty,
        sys::kJSTypedArrayTypeInt8Array
            | sys::kJSTypedArrayTypeUint8Array
            | sys::kJSTypedArrayTypeUint8ClampedArray
            | sys::kJSTypedArrayTypeInt16Array
            | sys::kJSTypedArrayTypeUint16Array
            | sys::kJSTypedArrayTypeInt32Array
            | sys::kJSTypedArrayTypeUint32Array
            | sys::kJSTypedArrayTypeBigInt64Array
            | sys::kJSTypedArrayTypeBigUint64Array
    );
    if !integer {
        return Err(JsError::dom("TypeMismatchError", "The provided ArrayBufferView is not an integer-typed array."));
    }
    let mut exc = std::ptr::null();
    // SAFETY: integer typed array confirmed above; length then pointer are
    // read and the bytes filled before returning control to JS.
    let len = unsafe { sys::JSObjectGetTypedArrayByteLength(ctx, obj.as_raw(), &mut exc) };
    check_exc(s, exc)?;
    if len > MAX_RANDOM_BYTES {
        return Err(JsError::dom(
            "QuotaExceededError",
            format!("The ArrayBufferView's byte length ({len}) exceeds the number of bytes of entropy available via this API ({MAX_RANDOM_BYTES})."),
        ));
    }
    if len == 0 {
        return Ok(v);
    }
    let mut random = vec![0u8; len];
    fill_random(&mut random)?;
    let mut exc = std::ptr::null();
    // SAFETY: as above. JSObjectGetTypedArrayBytesPtr returns the start of the
    // backing store, so the view's byteOffset is added; the view spans `len`
    // bytes from there. No JS runs between obtaining the pointer and writing.
    unsafe {
        let offset = sys::JSObjectGetTypedArrayByteOffset(ctx, obj.as_raw(), &mut exc);
        check_exc(s, exc)?;
        let ptr = sys::JSObjectGetTypedArrayBytesPtr(ctx, obj.as_raw(), &mut exc) as *mut u8;
        check_exc(s, exc)?;
        if !ptr.is_null() {
            std::ptr::copy_nonoverlapping(random.as_ptr(), ptr.add(offset), len);
        }
    }
    Ok(v)
}

/// RFC 4122 version 4 UUID string.
pub(crate) fn random_uuid() -> JsResult<String> {
    let mut b = [0u8; 16];
    fill_random(&mut b)?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    Ok(format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]))
}

pub struct Crypto;
impl JsClass for Crypto {
    const NAME: &'static str = "Crypto";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.method("getRandomValues", 1, |s, _d, a| {
            a.require(1, "Crypto.getRandomValues")?;
            get_random_values(s, a.get(0))
        });
        def.method("randomUUID", 0, |s, _d, _a| Ok(s.string(&random_uuid()?)));
        def.getter("subtle", |s, _d| {
            let key = IdentityKey(1 << 55, 2);
            let page = crate::target::page_of(s)?;
            crate::window::same_object(s, &page, key, || s.wrap::<SubtleCrypto>((), Some(key)))
        });
    }
}

fn digest_name(s: &Scope<'_>, v: Value<'_>) -> JsResult<String> {
    let name = match v.as_object() {
        Some(o) if !v.is_string() => o.get_atom("name")?,
        _ => v,
    };
    if name.is_undefined() {
        return Err(JsError::Type("Algorithm: name is required".into()));
    }
    let _ = s;
    Ok(dom_string(name)?.to_ascii_uppercase())
}

fn rejected<'s>(s: &Scope<'s>, error: JsError) -> JsResult<Value<'s>> {
    let d = s.deferred()?;
    d.reject(s, error.to_value(s))?;
    Ok(d.promise.get(s))
}

pub struct SubtleCrypto;
impl JsClass for SubtleCrypto {
    const NAME: &'static str = "SubtleCrypto";
    type Data = ();
    type Parent = NoParent;
    fn define(def: &mut ClassDef<Self>) {
        def.static_method("supports", 2, |s, a| {
            a.require(2, "SubtleCrypto.supports")?;
            let page = crate::target::page_of(s)?;
            let module = subtle_module(s, &page)?;
            let args: Vec<_> = a.iter().collect();
            module.get_atom("supports")?.as_object().ok_or_else(unavailable)?.call(None, &args)
        });
        def.method("digest", 2, |s, _d, a| {
            // Argument errors reject the promise (Web Crypto §14.3.5).
            let result = (|| -> JsResult<Vec<u8>> {
                a.require(2, "SubtleCrypto.digest")?;
                let name = digest_name(s, a.get(0))?;
                let data = crate::encoding::buffer_source_bytes(s, a.get(1))?.ok_or_else(|| {
                    JsError::Type("Failed to execute 'digest' on 'SubtleCrypto': parameter 2 is not of type '(ArrayBuffer or ArrayBufferView)'.".into())
                })?;
                Ok(match name.as_str() {
                    "SHA-1" => Sha1::digest(&data).to_vec(),
                    "SHA-256" => Sha256::digest(&data).to_vec(),
                    "SHA-384" => Sha384::digest(&data).to_vec(),
                    "SHA-512" => Sha512::digest(&data).to_vec(),
                    _ => return Err(JsError::dom("NotSupportedError", format!("Algorithm '{name}' is not supported for digest"))),
                })
            })();
            match result {
                Ok(bytes) => {
                    let d = s.deferred()?;
                    let ab = crate::encoding::array_buffer(s, &bytes)?;
                    d.resolve(s, ab)?;
                    Ok(d.promise.get(s))
                }
                Err(JsError::Terminated) => Err(JsError::Terminated),
                Err(e) => rejected(s, e),
            }
        });
        macro_rules! operations {
            ($($m:literal: $length:literal),*) => {$(
                def.method($m, $length, |s, _d, a| subtle_call(s, $m, a));
            )*};
        }
        def.promise_method("getPublicKey", 2, |s, _d, a| subtle_call(s, "getPublicKey", a));
        operations!(
            "encrypt": 3,
            "decrypt": 3,
            "sign": 3,
            "verify": 4,
            "generateKey": 3,
            "deriveKey": 5,
            "deriveBits": 2,
            "importKey": 5,
            "exportKey": 2,
            "wrapKey": 4,
            "unwrapKey": 7
        );
    }
}

/// Kept on the realm's `navigator` (a hidden property), itself kept per realm.
const SUBTLE_KEY: &str = "lpSubtle";

fn unavailable() -> JsError {
    JsError::Type("Web Crypto is unavailable in this realm".into())
}

/// This realm's subtle.js module, made on first use.
fn subtle_module<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Object<'s>> {
    let holder = crate::worker_scope::module_holder(s, page)?;
    if let Some(module) = s.get_hidden(holder, SUBTLE_KEY)?.and_then(|v| v.as_object()) {
        return Ok(module);
    }
    let natives = Persistent::new(s, crate::crypto_subtle::natives(s)?.value());
    let factory = s.eval(include_str!("subtle.js"), Some("pyro://internal/subtle.js"), 1)?;
    let module = factory.as_object().ok_or_else(unavailable)?.call(None, &[natives.get(s)])?;
    s.set_hidden(holder, SUBTLE_KEY, module)?;
    module.as_object().ok_or_else(unavailable)
}

/// `crypto.subtle.<name>(...)`: a promise; argument errors reject it.
fn subtle_call<'s>(s: &Scope<'s>, name: &'static str, a: Args<'s>) -> JsResult<Value<'s>> {
    let result = (|| -> JsResult<Value<'s>> {
        let page = crate::target::page_of(s)?;
        let module = subtle_module(s, &page)?;
        let args: Vec<Value<'s>> = a.iter().collect();
        let args = s.array(&args)?;
        module.get_atom("call")?.as_object().ok_or_else(unavailable)?.call(None, &[s.string(name), args.value()])
    })();
    match result {
        Err(JsError::Terminated) => Err(JsError::Terminated),
        Err(JsError::ContextDestroyed) => Err(JsError::ContextDestroyed),
        Err(e) => rejected(s, e),
        ok => ok,
    }
}

/// Adds the lazy `CryptoKey` interface name to a window or worker global.
pub(crate) fn define_globals<W>(def: &mut ClassDef<W>)
where
    W: JsClass,
    W::Data: WindowHost,
{
    def.global_value_accessor("CryptoKey", |s, w| crypto_key(s, w.page()), |s, w, value| replace(s, w.page(), value));
}

fn crypto_key<'s>(s: &Scope<'s>, page: &Rc<PageDom>) -> JsResult<Value<'s>> {
    let value = subtle_module(s, page)?.get_atom("CryptoKey")?;
    replace(s, page, value)?;
    Ok(value)
}

/// Shadows the accessor with the interface object's usual data property.
fn replace(s: &Scope<'_>, page: &Rc<PageDom>, value: Value<'_>) -> JsResult<()> {
    let define = page.define_properties.borrow().get(&s.realm_id()).map(|p| p.get(s));
    let define = define.and_then(|v| v.as_object()).ok_or_else(unavailable)?;
    let desc = s.object();
    desc.set_atom("value", value)?;
    desc.set_atom("writable", s.boolean(true))?;
    desc.set_atom("enumerable", s.boolean(false))?;
    desc.set_atom("configurable", s.boolean(true))?;
    define.call(None, &[s.global().value(), s.string("CryptoKey"), desc.value()])?;
    Ok(())
}

/// Structured clone of a platform object the clone script does not know:
/// a copy when `value` is a CryptoKey of this realm, else undefined. A realm
/// that never made subtle.js has no CryptoKey, so this costs it one lookup.
pub(crate) fn clone_platform_object<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let holder = crate::worker_scope::module_holder(s, &page)?;
    let Some(module) = s.get_hidden(holder, SUBTLE_KEY)?.and_then(|v| v.as_object()) else {
        return Ok(s.undefined());
    };
    module.get_atom("cloneKey")?.as_object().ok_or_else(unavailable)?.call(None, &[value])
}

/// The cross-VM form of a CryptoKey of this realm (`worker_wire.js`):
/// `[type, extractable, algorithm, usages, key bytes]` as plain data, or
/// `None` when `value` is not one. Costs a realm without subtle.js one lookup.
pub(crate) fn serialize_key<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Option<Value<'s>>> {
    let page = crate::target::page_of(s)?;
    let holder = crate::worker_scope::module_holder(s, &page)?;
    let Some(module) = s.get_hidden(holder, SUBTLE_KEY)?.and_then(|v| v.as_object()) else {
        return Ok(None);
    };
    let parts = module.get_atom("serializeKey")?.as_object().ok_or_else(unavailable)?.call(None, &[value])?;
    Ok((!parts.is_undefined()).then_some(parts))
}

/// Resolve a genuine same-origin sibling key through its realm's private
/// brand and the normal CryptoKey serialization, preserving extractability.
pub(crate) fn foreign_key<'s>(s: &Scope<'s>, value: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    let held = Persistent::new(s, value);
    for realm in page.sibling_realms(s) {
        let Ok(owner) = realm.scope() else { continue };
        let Ok(other) = crate::target::page_of(&owner) else { continue };
        if other.is_disposed() || other.origin_key() != page.origin_key() { continue; }
        let Some(parts) = serialize_key(&owner, held.get(&owner))? else { continue };
        let parts = Persistent::new(&owner, parts);
        let local = crate::structured_clone::clone_value(s, parts.get(s), s.undefined())?;
        return deserialize_key(s, local);
    }
    Ok(s.undefined())
}

/// The CryptoKey, in this realm, of parts made by [`serialize_key`]
/// (making the realm's subtle.js if it has none yet).
pub(crate) fn deserialize_key<'s>(s: &Scope<'s>, parts: Value<'s>) -> JsResult<Value<'s>> {
    let page = crate::target::page_of(s)?;
    // CryptoKey is `[SecureContext, Serializable]`: an insecure receiver cannot deserialize one.
    if !crate::secure_context::secure(s, &page) {
        return Err(JsError::dom("DataCloneError", "A CryptoKey cannot be deserialized in an insecure context."));
    }
    let module = subtle_module(s, &page)?;
    module.get_atom("deserializeKey")?.as_object().ok_or_else(unavailable)?.call(None, &[parts])
}

/// `window.crypto` (one Crypto object per realm).
pub fn crypto_value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let key = IdentityKey(1 << 55, 1);
    let page = crate::target::page_of(s)?;
    crate::window::same_object(s, &page, key, || s.wrap::<Crypto>((), Some(key)))
}

pub(crate) fn register(engine: &pyro_js::Engine) {
    engine.register::<Crypto>();
    engine.register::<SubtleCrypto>();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn uuid_shape() {
        let u = random_uuid().unwrap();
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "4");
        assert!(matches!(&u[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(u, random_uuid().unwrap());
    }
}
