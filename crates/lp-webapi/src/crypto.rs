//! Web Crypto subset: `crypto.getRandomValues`, `crypto.randomUUID` and
//! `crypto.subtle.digest` (SHA-1/256/384/512). Other SubtleCrypto
//! operations reject with NotSupportedError (upstream implements more,
//! src/browser/webapi/SubtleCrypto.zig; not ported yet).
//!
//! `getRandomValues` writes into the caller's typed array through the JSC C
//! API (backing store pointer + byteOffset); the pointer is obtained and
//! used with no intervening JS call.

use lp_js::sys;
use lp_js::{ClassDef, IdentityKey, JsClass, JsError, JsResult, NoParent, Scope, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha384, Sha512};

use crate::util::*;

/// QuotaExceededError limit of `getRandomValues` (Web Crypto §10.1.1).
const MAX_RANDOM_BYTES: usize = 65536;

fn check_exc(s: &Scope<'_>, exc: sys::JSValueRef) -> JsResult<()> {
    if exc.is_null() {
        return Ok(());
    }
    // SAFETY: `exc` is the live exception JSC just returned for this context.
    let v = unsafe { Value::from_raw(s.ctx(), exc) };
    Err(JsError::Thrown(lp_js::Persistent::new(s, v)))
}

fn fill_random(buf: &mut [u8]) -> JsResult<()> {
    getrandom::fill(buf).map_err(|e| JsError::dom("OperationError", format!("random source failed: {e}")))
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
        Some(o) if !v.is_string() => o.get("name")?,
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
        macro_rules! unsupported {
            ($($m:literal),*) => {$(
                def.method($m, 0, |s, _d, _a| {
                    rejected(s, JsError::dom("NotSupportedError", concat!("SubtleCrypto.", $m, " is not implemented by lightningpanda")))
                });
            )*};
        }
        unsupported!(
            "encrypt",
            "decrypt",
            "sign",
            "verify",
            "generateKey",
            "deriveKey",
            "deriveBits",
            "importKey",
            "exportKey",
            "wrapKey",
            "unwrapKey"
        );
    }
}

/// `window.crypto` (one Crypto object per realm).
pub fn crypto_value<'s>(s: &Scope<'s>) -> JsResult<Value<'s>> {
    let key = IdentityKey(1 << 55, 1);
    let page = crate::target::page_of(s)?;
    crate::window::same_object(s, &page, key, || s.wrap::<Crypto>((), Some(key)))
}

pub(crate) fn register(engine: &lp_js::Engine) {
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
